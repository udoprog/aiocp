use crate::io::{Code, Overlapped, OverlappedResult, OverlappedState};
use crate::pool::BufferPool;
use crate::sys::AsRawSocket;
use crate::task::{Header, LockResult};
use std::convert::TryFrom as _;
use std::fmt;
use std::future::Future;
use std::io;
use std::mem;
use std::os::windows::io::FromRawSocket;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use winapi::shared::minwindef::FALSE;
use winapi::shared::winerror;
use winapi::um::errhandlingapi;
use winapi::um::ioapiset;
use winapi::um::minwinbase;

mod operation;
use self::operation::Operation;

mod ext;

/// A wrapped socket able to perform overlapped operations.
pub struct Socket<S>
where
    S: AsRawSocket,
{
    pub(crate) socket: S,
    pub(crate) port: crate::sys::CompletionPort,
    pub(crate) header: Arc<Header>,
}

impl<S> Socket<S>
where
    S: AsRawSocket,
{
    /// Construct a socket wrapper capable of overlapped operations.
    pub(crate) fn new(socket: S, port: crate::sys::CompletionPort, max_buffer_size: usize) -> Self {
        Self {
            socket,
            port,
            header: Arc::new(Header::new(max_buffer_size)),
        }
    }

    /// Duplicate this socket, allowing more than one pending I/O operation to
    /// happen over it concurrently.
    ///
    /// The underlying socket must implement [Clone], which it can trivially do
    /// if it's wrapped in something like an [Arc].
    ///
    /// The returned file socket does not inherit the state of the socket it was
    /// cloned from. It has no pending operations. It also doesn't cause any
    /// racing with the current socket and is free to perform other kinds of
    /// operations.
    pub fn duplicate(&self) -> Self
    where
        S: Clone,
    {
        let max_buffer_size = self.header.pool.max_buffer_size();

        Self {
            socket: self.socket.clone(),
            port: self.port.clone(),
            header: Arc::new(Header::new(max_buffer_size)),
        }
    }

    /// Run the given I/O operation while managing it's overlapped
    /// notifications.
    ///
    /// If the operation indicates that it needs to block, the returned future
    /// will complete once the operation has completed.
    pub async fn run_operation<O>(&mut self, op: O) -> io::Result<O::Output>
    where
        O: Operation<S>,
    {
        RunOperation::new(self, op).await
    }

    /// Accept a new connection and return the socket associated with the
    /// client.
    pub async fn accept(&mut self) -> io::Result<Self>
    where
        S: FromRawSocket,
    {
        self.run_operation(operation::Accept::new()).await
    }

    /// Access a reference to the underlying socket.
    pub fn as_ref(&self) -> &S {
        &self.socket
    }

    /// Access a mutable reference to the underlying socket.
    pub fn as_mut(&mut self) -> &mut S {
        &mut self.socket
    }

    /// Register a new waker.
    pub(crate) fn register_by_ref(&self, waker: &Waker) {
        self.header.register_by_ref(waker);
    }

    /// Lock the current socket.
    pub(crate) fn lock<'a>(
        &'a mut self,
        code: Code,
        waker: &Waker,
    ) -> io::Result<Option<LockGuard<'a, S>>> {
        let permit = self.port.permit()?;
        self.register_by_ref(waker);

        Ok(match self.header.header_lock(code) {
            LockResult::Ok(state) => Some(LockGuard {
                socket: &mut self.socket,
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
                self.socket.as_raw_socket() as *mut _,
                self.header.as_raw_overlapped(),
            );
        }
    }
}

unsafe impl<S> Send for Socket<S> where S: AsRawSocket + Send {}
unsafe impl<S> Sync for Socket<S> where S: AsRawSocket + Sync {}

impl<S> fmt::Debug for Socket<S>
where
    S: AsRawSocket,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Socket")
            .field("socket", &self.socket.as_raw_socket())
            .finish()
    }
}

impl<S> Drop for Socket<S>
where
    S: AsRawSocket,
{
    fn drop(&mut self) {
        self.cancel_if_pending();
    }
}

/// The state of the operation being run.
#[derive(Debug)]
enum State<'a, S, O>
where
    S: AsRawSocket,
    O: Operation<S>,
{
    /// The operation is in its initial state.
    Running { io: &'a mut Socket<S>, op: O },
    /// State of the task is completed.
    Complete,
}

/// An operation performed over the completion port.
pub(crate) struct RunOperation<'a, S, O>
where
    S: AsRawSocket,
    O: Operation<S>,
{
    state: State<'a, S, O>,
}

impl<'a, S, O> RunOperation<'a, S, O>
where
    S: AsRawSocket,
    O: Operation<S>,
{
    /// Construct a new future running the given operation.
    pub(crate) fn new(io: &'a mut Socket<S>, op: O) -> Self {
        Self {
            state: State::Running { io, op },
        }
    }
}

impl<'a, S, O> Future for RunOperation<'a, S, O>
where
    S: AsRawSocket,
    O: Operation<S>,
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
                        let (pool, mut overlapped, socket) = guard.prepare();
                        let result = op.prepare(socket, &mut overlapped, pool);
                        crate::io::handle_io_pending(result)?;
                        std::mem::forget((guard, overlapped));
                        return Poll::Pending;
                    }
                    OverlappedState::Pending => {
                        let result = guard.result()?;
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

/// A lock guard that will unlock the resource if dropped.
///
/// Dropping the guard automatically clears the available buffers and unlocks
/// the header.
pub(crate) struct LockGuard<'a, S> {
    socket: &'a mut S,
    header: &'a Arc<Header>,
    state: OverlappedState,
    _permit: crate::sys::CompletionPortPermit<'a>,
}

impl<'a, S> LockGuard<'a, S>
where
    S: AsRawSocket,
{
    /// Get the overlapped result from the current structure.
    pub(crate) fn result(&self) -> io::Result<OverlappedResult> {
        // Safety: All of there operations are safe and it is guaranteed that
        // we have access to the underlying handles.
        unsafe {
            let mut bytes_transferred = mem::MaybeUninit::zeroed();

            let result = ioapiset::GetOverlappedResult(
                self.socket.as_raw_socket() as *mut _,
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

impl<S> LockGuard<'_, S> {
    /// Prepare the information.
    pub fn prepare(&mut self) -> (&BufferPool, Overlapped, &mut S) {
        let pool = &self.header.pool;
        pool.clear();

        let overlapped = Overlapped::from_raw(Arc::into_raw(self.header.clone())
            as *const minwinbase::OVERLAPPED
            as *mut _);

        (pool, overlapped, &mut *self.socket)
    }

    /// Get the state of the guard.
    pub fn state(&self) -> OverlappedState {
        self.state
    }

    /// Access the pool associated with the locked state of the header.
    pub fn pool(&self) -> &BufferPool {
        &self.header.pool
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

impl<S> Drop for LockGuard<'_, S> {
    fn drop(&mut self) {
        self.header.pool.clear();
        self.header.unlock_to_open();
    }
}
