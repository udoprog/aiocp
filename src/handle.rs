use crate::io::OverlappedState;
use crate::ioctl;
use crate::operation::{self, Operation};
use crate::operation_poller::OperationPoller;
use crate::task::Header;
use std::convert::TryFrom as _;
use std::fmt;
use std::future::Future;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Waker;
use std::task::{Context, Poll};
use winapi::shared::minwindef::FALSE;
use winapi::shared::winerror;
use winapi::um::errhandlingapi;
use winapi::um::ioapiset;

/// The results of an overlapped operation.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct OverlappedResult {
    /// The bytes transferred by this operation. Its exact meaning is determined
    /// by [Operation::collect].
    pub bytes_transferred: usize,
}

impl OverlappedResult {
    /// An empty overlapped result.
    pub(crate) const fn empty() -> Self {
        Self {
            bytes_transferred: 0,
        }
    }
}

/// A wrapped handle able to perform overlapped operations.
pub struct Handle<H>
where
    H: AsRawHandle,
{
    pub(crate) handle: H,
    pub(crate) port: crate::sys::CompletionPort,
    pub(crate) header: Arc<Header>,
}

unsafe impl<H> Send for Handle<H> where H: AsRawHandle + Send {}
unsafe impl<H> Sync for Handle<H> where H: AsRawHandle + Sync {}

impl<H> Handle<H>
where
    H: AsRawHandle,
{
    /// Construct a zeroed overlapped structure with the given buffers available
    /// for performing operations over.
    pub(crate) fn new(handle: H, port: crate::sys::CompletionPort) -> Self {
        Self {
            handle,
            port,
            header: Arc::new(Header::new()),
        }
    }

    /// Duplicate this handle, allowing more than one pending I/O operation to
    /// happen over it concurrently.
    ///
    /// The underlying handle must implement [Clone], which it can trivially do
    /// if it's wrapped in something like an [Arc].
    ///
    /// The returned file handle does not inherit the state of the handle it was
    /// cloned from. It has no pending operations. It also doesn't cause any
    /// racing with the current handle and is free to perform other kinds of
    /// operations.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use aiocp::ArcHandle;
    /// use std::fs::OpenOptions;
    /// use std::io;
    /// use std::os::windows::fs::OpenOptionsExt as _;
    /// use std::sync::Arc;
    /// use tokio::io::AsyncReadExt as _;
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let (port, handle) = aiocp::setup(2)?;
    ///
    /// let output = OpenOptions::new()
    ///     .read(true)
    ///     .custom_flags(aiocp::flags::FILE_FLAG_OVERLAPPED)
    ///     .open("file.txt")?;
    ///
    /// let output = ArcHandle::new(output);
    ///
    /// let mut reader1 = port.register(output, 0)?;
    /// let mut reader2 = reader1.duplicate();
    ///
    /// let mut buf1 = Vec::new();
    /// let read1 = reader1.read_to_end(&mut buf1);
    ///
    /// let mut buf2 = Vec::new();
    /// let read2 = reader2.read_to_end(&mut buf2);
    ///
    /// let (n1, n2) = tokio::try_join!(read1, read2)?;
    /// assert_eq!(n1, n2);
    /// # Ok(()) }
    /// ```
    pub fn duplicate(&self) -> Self
    where
        H: Clone,
    {
        Self {
            handle: self.handle.clone(),
            port: self.port.clone(),
            header: Arc::new(Header::new()),
        }
    }

    /// Run the given I/O operation while managing it's overlapped
    /// notifications.
    ///
    /// If the operation indicates that it needs to block, the returned future
    /// will complete once the operation has completed.
    pub async fn run<O>(&mut self, op: O) -> io::Result<O::Output>
    where
        H: AsRawHandle,
        O: Operation<H>,
    {
        Run::new(self, op).await
    }

    /// Cancel if we know there's an operation running in the background.
    pub fn cancel_if_pending(&self) {
        if let OverlappedState::Complete = self.header.state() {
            self.cancel_immediate();
        }
    }

    /// Access a reference to the underlying handle.
    pub fn as_ref(&self) -> &H {
        &self.handle
    }

    /// Access a mutable reference to the underlying handle.
    pub fn as_mut(&mut self) -> &mut H {
        &mut self.handle
    }

    /// Write the given buffer and return the number of bytes written.
    pub async fn write<B>(&mut self, buf: B) -> io::Result<usize>
    where
        B: AsRef<[u8]>,
    {
        self.run(operation::Write::new(buf)).await
    }

    /// Write the given buffer and return the number of bytes written.
    pub async fn read<B>(&mut self, buf: B) -> io::Result<usize>
    where
        B: AsMut<[u8]>,
    {
        self.run(operation::Read::new(buf)).await
    }

    /// Connect the current handle, under the assumption that it is the server
    /// side of a named pipe.
    pub async fn connect_named_pipe(&mut self) -> io::Result<()> {
        self.run(operation::ConnectNamedPipe::new()).await
    }

    /// Connect the current handle, under the assumption that it is the server
    /// side of a named pipe.
    pub async fn device_io_control<M>(&mut self, message: M) -> io::Result<usize>
    where
        M: ioctl::Ioctl,
    {
        self.run(operation::DeviceIoCtl::new(message)).await
    }

    /// Get the overlapped result from the current structure.
    pub(crate) fn result(&self) -> io::Result<OverlappedResult>
    where
        H: AsRawHandle,
    {
        // Safety: All of there operations are safe and it is guaranteed that
        // we have access to the underlying handles.
        unsafe {
            let mut bytes_transferred = mem::MaybeUninit::zeroed();

            let result = ioapiset::GetOverlappedResult(
                self.handle.as_raw_handle() as *mut _,
                self.header.as_raw_overlapped(),
                bytes_transferred.as_mut_ptr(),
                FALSE,
            );

            if result == FALSE {
                return match errhandlingapi::GetLastError() {
                    winerror::ERROR_HANDLE_EOF => Ok(OverlappedResult::empty()),
                    winerror::ERROR_BROKEN_PIPE => Ok(OverlappedResult::empty()),
                    other => Err(io::Error::from_raw_os_error(other as i32)),
                };
            }

            let bytes_transferred = usize::try_from(bytes_transferred.assume_init())
                .expect("bytes transferred out of bounds");

            Ok(OverlappedResult { bytes_transferred })
        }
    }

    /// Register a new waker.
    pub(crate) fn register_by_ref(&self, waker: &Waker) {
        self.header.register_by_ref(waker);
    }

    /// Asynchronously cancel_if_pending an pending I/O operation associated with this
    /// handle on the I/O completion port.
    ///
    /// # Safety
    ///
    /// Issue a cancellation.
    pub(crate) fn cancel_immediate(&self) {
        // Safety: there's nothing inherently unsafe about this, both the header
        // pointer and the local handle are guaranteed to be alive for the
        // duration of this object.
        unsafe {
            let _ = ioapiset::CancelIoEx(
                self.handle.as_raw_handle() as *mut _,
                self.header.as_raw_overlapped(),
            );
        }
    }
}

impl<H> fmt::Debug for Handle<H>
where
    H: AsRawHandle,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field("handle", &self.handle.as_raw_handle())
            .finish()
    }
}

impl<H> Drop for Handle<H>
where
    H: AsRawHandle,
{
    fn drop(&mut self) {
        self.cancel_if_pending();
    }
}

/// The state of the operation being run.
#[derive(Debug)]
enum State<'a, H, O>
where
    H: AsRawHandle,
    O: Operation<H>,
{
    /// The operation is in its initial state.
    Running { op: OperationPoller<'a, H, O> },
    /// State of the task is completed.
    Complete,
}

/// An operation performed over the completion port.
pub(crate) struct Run<'a, H, O>
where
    H: AsRawHandle,
    O: Operation<H>,
{
    state: State<'a, H, O>,
}

impl<'a, H, O> Run<'a, H, O>
where
    H: AsRawHandle,
    O: Operation<H>,
{
    /// Construct a new future running the given operation.
    pub(crate) fn new(io: &'a mut Handle<H>, op: O) -> Self {
        Self {
            state: State::Running {
                op: OperationPoller::new(io, op),
            },
        }
    }
}

impl<'a, H, O> Future for Run<'a, H, O>
where
    H: AsRawHandle,
    O: Operation<H>,
{
    type Output = io::Result<O::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        match &mut this.state {
            State::Running { op } => {
                let output = match op.poll(cx) {
                    Poll::Ready(output) => output,
                    Poll::Pending => return Poll::Pending,
                };

                this.state = State::Complete;
                Poll::Ready(output)
            }
            State::Complete => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::Other,
                "polled after completion",
            ))),
        }
    }
}

/// Helper to deal with I/O pending errors correctly.
///
/// Returns Some(e) if an immediate error should be returned.
pub(crate) fn handle_io_pending<O>(result: io::Result<O>) -> io::Result<()> {
    match result {
        Ok(..) => (),
        Err(e) if e.raw_os_error() == Some(crate::errors::ERROR_IO_PENDING) => (),
        Err(e) => return Err(e),
    }

    Ok(())
}
