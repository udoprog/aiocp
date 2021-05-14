use crate::completion_port::CompletionPort;
use crate::io::{OverlappedHeader, OverlappedResult};
use crate::ioctl;
use crate::operation::Operation;
use crate::ops::{self, IocpOperation};
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
use winapi::um::ioapiset;

/// A wrapped handle able to perform overlapped operations.
pub struct OverlappedHandle<H> {
    pub(crate) handle: H,
    pub(crate) port: CompletionPort,
    pub(crate) header: Arc<OverlappedHeader>,
}

unsafe impl<H> Send for OverlappedHandle<H> where H: Send {}
unsafe impl<H> Sync for OverlappedHandle<H> where H: Sync {}

impl<H> OverlappedHandle<H> {
    /// Construct a zeroed overlapped structure with the given buffers available
    /// for performing operations over.
    pub(crate) fn new(handle: H, port: CompletionPort) -> Self {
        Self {
            handle,
            port,
            header: Arc::new(OverlappedHeader::new()),
        }
    }

    /// Run the given I/O operation while managing it's overlapped
    /// notifications.
    ///
    /// If the operation indicates that it needs to block, the returned future
    /// will complete once the operation has completed.
    pub fn run<O>(&mut self, op: O) -> IocpRun<'_, H, O>
    where
        H: AsRawHandle,
        O: IocpOperation<H>,
    {
        IocpRun::new(self, op)
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
                return Err(io::Error::last_os_error());
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

/// The state of the operation being run.
#[derive(Debug)]
enum State<'a, H, O>
where
    H: AsRawHandle,
{
    /// The operation is in its initial state.
    Running { op: Operation<'a, H, O> },
    /// State of the task is completed.
    Complete,
}

/// An operation performed over the completion port.
pub struct IocpRun<'a, H, O>
where
    H: AsRawHandle,
{
    state: State<'a, H, O>,
}

impl<'a, H, O> IocpRun<'a, H, O>
where
    H: AsRawHandle,
    O: IocpOperation<H>,
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

impl<'a, H, O> Future for IocpRun<'a, H, O>
where
    H: AsRawHandle,
    O: IocpOperation<H>,
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
