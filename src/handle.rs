use crate::completion_port::{CompletionPort, CompletionPortPermit};
use crate::io::{Code, Overlapped, OverlappedResult, OverlappedState};
use crate::ioctl;
use crate::pool::BufferPool;
use crate::task::{Header, LockResult};
use std::convert::TryFrom as _;
use std::fmt;
use std::future::Future;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use winapi::shared::minwindef::FALSE;
use winapi::shared::winerror;
use winapi::um::errhandlingapi;
use winapi::um::ioapiset;
use winapi::um::minwinbase;

mod operation;
use self::operation::Operation;

mod ext;
use self::ext::HandleExt as _;

/// A wrapped handle able to perform overlapped operations.
pub struct Handle<H>
where
    H: AsRawHandle,
{
    pub(crate) handle: H,
    pub(crate) port: CompletionPort,
    pub(crate) header: Arc<Header>,
    pool: BufferPool,
}

impl<H> Handle<H>
where
    H: AsRawHandle,
{
    /// Construct a handle wrapper capable of overlapped operations.
    pub(crate) fn new(handle: H, port: CompletionPort, max_buffer_size: usize) -> Self {
        Self {
            handle,
            port,
            header: Arc::new(Header::new()),
            pool: BufferPool::new(max_buffer_size),
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
    /// use iocp::ArcHandle;
    /// use std::fs::OpenOptions;
    /// use std::io;
    /// use std::os::windows::fs::OpenOptionsExt as _;
    /// use std::sync::Arc;
    /// use tokio::io::AsyncReadExt as _;
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let (port, handle) = iocp::setup(1)?;
    ///
    /// let output = OpenOptions::new()
    ///     .read(true)
    ///     .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
    ///     .open("file.txt")?;
    ///
    /// let output = ArcHandle::new(output);
    ///
    /// let mut reader1 = port.register_handle(output, Default::default())?;
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
        let max_buffer_size = self.pool.max_buffer_size();

        Self {
            handle: self.handle.clone(),
            port: self.port.clone(),
            header: Arc::new(Header::new()),
            pool: BufferPool::new(max_buffer_size),
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

    /// Register a new waker.
    pub(crate) fn register_by_ref(&self, waker: &Waker) {
        self.header.register_by_ref(waker);
    }

    /// Lock the current handle.
    pub(crate) fn lock<'a>(
        &'a mut self,
        code: Code,
        waker: &Waker,
    ) -> io::Result<Option<LockGuard<'a, H>>> {
        let permit = self.port.permit()?;
        self.register_by_ref(waker);

        Ok(match self.header.header_lock(code) {
            LockResult::Ok(state) => Some(LockGuard {
                handle: &mut self.handle,
                pool: &mut self.pool,
                header: &self.header,
                state,
                _permit: permit,
            }),
            LockResult::Busy(mismatch) => {
                if mismatch {
                    self.cancel_immediate();
                }

                None
            }
        })
    }

    /// Cancel if we know there's an operation running in the background.
    pub(crate) fn cancel_if_pending(&self) {
        if let OverlappedState::Pending = self.header.state() {
            self.cancel_immediate();
        }
    }

    /// Asynchronously cancel_if_pending an pending I/O operation associated with this
    /// handle on the I/O completion port.
    ///
    /// # Safety
    ///
    /// Issue a cancellation.
    fn cancel_immediate(&self) {
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

    /// Run the given I/O operation while managing it's overlapped
    /// notifications.
    ///
    /// If the operation indicates that it needs to block, the returned future
    /// will complete once the operation has completed.
    pub async fn run_operation<O>(&mut self, op: O) -> io::Result<O::Output>
    where
        O: Operation<H>,
    {
        RunOperation::new(self, op).await
    }

    /// Write the given buffer and return the number of bytes written.
    pub async fn write<B>(&mut self, buf: B) -> io::Result<usize>
    where
        B: AsRef<[u8]>,
    {
        self.run_operation(operation::Write::new(buf)).await
    }

    /// Write the given buffer and return the number of bytes written.
    pub async fn read<B>(&mut self, buf: B) -> io::Result<usize>
    where
        B: AsMut<[u8]>,
    {
        self.run_operation(operation::Read::new(buf)).await
    }

    /// Connect the current handle, under the assumption that it is the server
    /// side of a named pipe.
    pub async fn connect_named_pipe(&mut self) -> io::Result<()> {
        self.run_operation(operation::ConnectNamedPipe::new()).await
    }

    /// Connect the current handle, under the assumption that it is the server
    /// side of a named pipe.
    pub async fn device_io_control<M>(&mut self, message: M) -> io::Result<usize>
    where
        M: ioctl::Ioctl,
    {
        self.run_operation(operation::DeviceIoCtl::new(message))
            .await
    }

    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>>
    where
        H: AsRawHandle,
    {
        trace!(op = "read", "poll");

        let mut guard = match self.lock(operation::READ, cx.waker())? {
            Some(guard) => guard,
            None => return Poll::Pending,
        };

        trace!(op = "read", state = ?guard.state(), "unlocked");

        match guard.state() {
            OverlappedState::Idle => {
                let (pool, mut overlapped, handle) = guard.prepare();
                let mut b = pool.take(buf.remaining());
                let result = handle.read_overlapped(&mut b, &mut overlapped);
                crate::io::handle_io_pending(result)?;
                std::mem::forget((guard, overlapped));
                Poll::Pending
            }
            OverlappedState::Pending => {
                let result = guard.handle_result()?;
                let pool = guard.pool();
                let b = pool.release(result.bytes_transferred);
                let filled = b.filled();
                buf.put_slice(filled);
                guard.advance(filled.len());
                Poll::Ready(Ok(()))
            }
        }
    }

    fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>
    where
        H: AsRawHandle,
    {
        trace!(op = "write", "poll");

        let mut guard = match self.lock(operation::WRITE, cx.waker())? {
            Some(guard) => guard,
            None => return Poll::Pending,
        };

        trace!(op = "write", state = ?guard.state(), "unlocked");

        match guard.state() {
            OverlappedState::Idle => {
                let (pool, mut overlapped, handle) = guard.prepare();
                let mut b = pool.take(buf.len());
                b.put_slice(buf);
                let result = handle.write_overlapped(b.filled(), &mut overlapped);
                crate::io::handle_io_pending(result)?;
                std::mem::forget((guard, overlapped));
                Poll::Pending
            }
            OverlappedState::Pending => {
                let result = guard.handle_result()?;
                guard.advance(result.bytes_transferred);
                Poll::Ready(Ok(result.bytes_transferred))
            }
        }
    }
}

unsafe impl<H> Send for Handle<H> where H: AsRawHandle + Send {}
unsafe impl<H> Sync for Handle<H> where H: AsRawHandle + Sync {}

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
    Running { io: &'a mut Handle<H>, op: O },
    /// State of the task is completed.
    Complete,
}

/// An operation performed over the completion port.
pub(crate) struct RunOperation<'a, H, O>
where
    H: AsRawHandle,
    O: Operation<H>,
{
    state: State<'a, H, O>,
}

impl<'a, H, O> RunOperation<'a, H, O>
where
    H: AsRawHandle,
    O: Operation<H>,
{
    /// Construct a new future running the given operation.
    pub(crate) fn new(io: &'a mut Handle<H>, op: O) -> Self {
        Self {
            state: State::Running { io, op },
        }
    }
}

impl<'a, H, O> Future for RunOperation<'a, H, O>
where
    H: AsRawHandle,
    O: Operation<H>,
{
    type Output = io::Result<O::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        let output = match &mut this.state {
            State::Running { io, op } => {
                let mut guard = match io.lock(O::CODE, cx.waker())? {
                    Some(guard) => guard,
                    None => return Poll::Pending,
                };

                let output = match guard.state() {
                    OverlappedState::Idle => {
                        let (pool, mut overlapped, handle) = guard.prepare();
                        let result = op.prepare(handle, &mut overlapped, pool);
                        crate::io::handle_io_pending(result)?;
                        std::mem::forget((guard, overlapped));
                        return Poll::Pending;
                    }
                    OverlappedState::Pending => {
                        let result = guard.handle_result()?;
                        let (output, outcome) = op.collect(result, guard.pool())?;
                        outcome.apply_to(&guard);
                        output
                    }
                };

                output
            }
            State::Complete => {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::Other,
                    "polled after completion",
                )))
            }
        };

        this.state = State::Complete;
        Poll::Ready(Ok(output))
    }
}

impl<H> AsyncRead for Handle<H>
where
    H: AsRawHandle,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = unsafe { self.get_unchecked_mut() };
        this.poll_read(cx, buf)
    }
}

impl<H> AsyncWrite for Handle<H>
where
    H: AsRawHandle,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let this = unsafe { self.get_unchecked_mut() };
        this.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// A lock guard that will unlock the resource if dropped.
///
/// Dropping the guard automatically clears the available buffers and unlocks
/// the header.
pub(crate) struct LockGuard<'a, H> {
    handle: &'a mut H,
    pool: &'a BufferPool,
    header: &'a Arc<Header>,
    state: OverlappedState,
    _permit: CompletionPortPermit<'a>,
}

impl<'a, H> LockGuard<'a, H>
where
    H: AsRawHandle,
{
    /// Get the overlapped result from the current structure.
    pub(crate) fn handle_result(&self) -> io::Result<OverlappedResult> {
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
}

impl<H> LockGuard<'_, H> {
    /// Prepare the information.
    pub fn prepare(&mut self) -> (&BufferPool, Overlapped, &mut H) {
        self.pool.clear();

        let overlapped = Overlapped::from_raw(Arc::into_raw(self.header.clone())
            as *const minwinbase::OVERLAPPED
            as *mut _);

        (&*self.pool, overlapped, &mut *self.handle)
    }

    /// Get the state of the guard.
    pub fn state(&self) -> OverlappedState {
        self.state
    }

    /// Access the pool associated with the locked state of the header.
    pub fn pool(&self) -> &BufferPool {
        &*self.pool
    }

    /// Advance the cursor of the overlapped structure by `amount`.
    ///
    /// This is used by read or write operations to move the read or write
    /// cursor forward.
    pub fn advance(&self, amount: usize) {
        use std::convert::TryFrom as _;

        trace!(amount = amount, "advance");

        // Safety: this is done in the guard, which ensures exclusive access.
        unsafe {
            let overlapped = &mut *self.header.raw.get();
            let s = overlapped.u.s_mut();
            let mut n = s.Offset as u64 | (s.OffsetHigh as u64) << 32;

            n = u64::try_from(amount)
                .ok()
                .and_then(|amount| n.checked_add(amount))
                .expect("advance out of bounds");

            s.Offset = (n & !0u32 as u64) as u32;
            s.OffsetHigh = (n >> 32) as u32;
        }
    }
}

impl<H> Drop for LockGuard<'_, H> {
    fn drop(&mut self) {
        self.pool.clear();
        self.header.unlock_to_open();
    }
}
