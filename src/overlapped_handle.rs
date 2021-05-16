use crate::io::{OverlappedHeader, OverlappedState};
use crate::ioctl;
use crate::operation::Operation;
use crate::ops::{self, OverlappedOperation};
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
    pub bytes_transferred: usize,
}

impl OverlappedResult {
    /// Construct an empty overlapped result.
    pub(crate) const fn empty() -> Self {
        Self {
            bytes_transferred: 0,
        }
    }
}

/// A wrapped handle able to perform overlapped operations.
pub struct OverlappedHandle<H>
where
    H: AsRawHandle,
{
    pub(crate) handle: H,
    pub(crate) port: crate::sys::CompletionPort,
    pub(crate) header: Arc<OverlappedHeader>,
}

unsafe impl<H> Send for OverlappedHandle<H> where H: AsRawHandle + Send {}
unsafe impl<H> Sync for OverlappedHandle<H> where H: AsRawHandle + Sync {}

impl<H> OverlappedHandle<H>
where
    H: AsRawHandle,
{
    /// Construct a zeroed overlapped structure with the given buffers available
    /// for performing operations over.
    pub(crate) fn new(handle: H, port: crate::sys::CompletionPort) -> Self {
        Self {
            handle,
            port,
            header: Arc::new(OverlappedHeader::new()),
        }
    }

    /// Helper to deal with I/O pending errors correctly.
    ///
    /// Returns Some(e) if an immediate error should be returned.
    pub(crate) fn handle_io_pending<O>(&self, result: io::Result<O>) -> io::Result<()> {
        match result {
            Ok(..) => (),
            Err(e) if e.raw_os_error() == Some(crate::errors::ERROR_IO_PENDING) => (),
            Err(e) => return Err(e),
        }

        Ok(())
    }

    /// Run the given I/O operation while managing it's overlapped
    /// notifications.
    ///
    /// If the operation indicates that it needs to block, the returned future
    /// will complete once the operation has completed.
    pub async fn run<O>(&mut self, op: O) -> io::Result<O::Output>
    where
        H: AsRawHandle,
        O: OverlappedOperation<H>,
    {
        Run::new(self, op).await
    }

    /// Access a reference to the underlying handle.
    pub fn as_ref(&self) -> &H {
        &self.handle
    }

    /// Access a mutable reference to the underlying handle.
    pub fn as_mut(&mut self) -> &mut H {
        &mut self.handle
    }

    /// Get the overlapped result from the current structure.
    ///
    /// # Safety
    ///
    /// This must only be called while under an exclusive [lock].
    pub(crate) fn result(&self) -> io::Result<OverlappedResult>
    where
        H: AsRawHandle,
    {
        unsafe {
            let mut bytes_transferred = mem::MaybeUninit::zeroed();

            let result = ioapiset::GetOverlappedResult(
                self.handle.as_raw_handle() as *mut _,
                self.header.raw.get(),
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
        self.header.waker.register_by_ref(waker);
    }
}

impl<H> OverlappedHandle<H>
where
    H: AsRawHandle,
{
    /// Asynchronously cancel an pending I/O operation associated with this
    /// handle on the I/O completion port.
    pub(crate) fn cancel(&self) {
        unsafe {
            let _ =
                ioapiset::CancelIoEx(self.handle.as_raw_handle() as *mut _, self.header.raw.get());
        }
    }

    /// Write the given buffer and return the number of bytes written.
    pub async fn write<B>(&mut self, buf: B) -> io::Result<usize>
    where
        B: AsRef<[u8]>,
    {
        self.run(ops::Write::new(buf)).await
    }

    /// Write the given buffer and return the number of bytes written.
    pub async fn read<B>(&mut self, buf: B) -> io::Result<usize>
    where
        B: AsMut<[u8]>,
    {
        self.run(ops::Read::new(buf)).await
    }

    /// Connect the current handle, under the assumption that it is the server
    /// side of a named pipe.
    pub async fn connect_named_pipe(&mut self) -> io::Result<()> {
        self.run(ops::ConnectNamedPipe::new()).await
    }

    /// Connect the current handle, under the assumption that it is the server
    /// side of a named pipe.
    pub async fn device_io_control<M>(&mut self, message: M) -> io::Result<usize>
    where
        M: ioctl::Ioctl,
    {
        self.run(ops::DeviceIoCtl::new(message)).await
    }
}

impl<H> fmt::Debug for OverlappedHandle<H>
where
    H: AsRawHandle,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlappedHandle")
            .field("handle", &self.handle.as_raw_handle())
            .finish()
    }
}

impl<H> Drop for OverlappedHandle<H>
where
    H: AsRawHandle,
{
    fn drop(&mut self) {
        if let OverlappedState::Remote = self.header.state() {
            self.cancel();
        }
    }
}

/// The state of the operation being run.
#[derive(Debug)]
enum State<'a, H, O>
where
    H: AsRawHandle,
    O: OverlappedOperation<H>,
{
    /// The operation is in its initial state.
    Running { op: Operation<'a, H, O> },
    /// State of the task is completed.
    Complete,
}

/// An operation performed over the completion port.
pub struct Run<'a, H, O>
where
    H: AsRawHandle,
    O: OverlappedOperation<H>,
{
    state: State<'a, H, O>,
}

impl<'a, H, O> Run<'a, H, O>
where
    H: AsRawHandle,
    O: OverlappedOperation<H>,
{
    /// Construct a new future running the given operation.
    pub(crate) fn new(io: &'a mut OverlappedHandle<H>, op: O) -> Self {
        Self {
            state: State::Running {
                op: Operation::new(io, op),
            },
        }
    }
}

impl<'a, H, O> Future for Run<'a, H, O>
where
    H: AsRawHandle,
    O: OverlappedOperation<H>,
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
