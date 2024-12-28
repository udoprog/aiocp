use crate::handle::Handle;
use crate::socket::Socket;
use crate::task::Header;
use std::fmt;
use std::io;
use std::mem;
use std::os::windows::io::{AsRawHandle, AsRawSocket};
use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use winapi::shared::minwindef::FALSE;
use winapi::shared::winerror;
use winapi::um::errhandlingapi;
use winapi::um::handleapi;
use winapi::um::ioapiset;
use winapi::um::winbase;
use winapi::um::winnt::HANDLE;

/// Default max buffer size in use.
const DEFAULT_MAX_BUFFER_SIZE: usize = 1 << 16;

/// Options to use when registering a handle or a socket.
#[derive(Debug, Clone, Copy)]
pub struct RegisterOptions {
    key: usize,
    max_buffer_size: usize,
}

impl RegisterOptions {
    /// Return a modified register options with a different max buffer size.
    pub fn with_max_buffer_size(self, max_buffer_size: usize) -> Self {
        Self {
            max_buffer_size,
            ..self
        }
    }
}

impl Default for RegisterOptions {
    fn default() -> Self {
        Self {
            key: 0,
            max_buffer_size: DEFAULT_MAX_BUFFER_SIZE,
        }
    }
}

/// The asynchronous handler for a Windows I/O Completion Port.
///
/// Permits the user to use it and wait for an overlapped result to complete.
#[derive(Clone)]
pub struct CompletionPort {
    inner: Arc<Inner>,
}

impl CompletionPort {
    /// Create a new completion port.
    pub fn create(threads: u32) -> io::Result<Self> {
        unsafe {
            let handle = ioapiset::CreateIoCompletionPort(
                handleapi::INVALID_HANDLE_VALUE,
                ptr::null_mut(),
                0,
                threads,
            );

            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }

            Ok(Self {
                inner: Arc::new(Inner {
                    pending: AtomicIsize::new(0),
                    handle,
                }),
            })
        }
    }

    /// Acquire a permit from the completion port with the intention of
    /// performing I/O operations over it. The returned handle will release it
    /// from I/O unless it is forgotten.
    pub fn permit(&self) -> io::Result<CompletionPortPermit<'_>> {
        let pending = self.inner.pending.fetch_add(1, Ordering::SeqCst);
        trace!(pending = pending, "acquiring permit");

        if pending >= 0 {
            return Ok(CompletionPortPermit { port: self });
        }

        let _pending = self.inner.pending.fetch_sub(1, Ordering::SeqCst);
        trace!(pending = _pending, "releasing failed permit");

        Err(io::Error::new(
            io::ErrorKind::Other,
            "port is shutting down",
        ))
    }

    /// Register the given handle for overlapped I/O and allocate buffers with
    /// the specified capacities that can be used inside of an operation with
    /// it.
    pub fn register_handle<H>(&self, handle: H, options: RegisterOptions) -> io::Result<Handle<H>>
    where
        H: AsRawHandle,
    {
        if options.key >= RESERVED_PORTS {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "trying to use reserved completion port",
            ));
        }

        // Safety: there's nothing inherently unsafe about this.
        unsafe {
            let handle = ioapiset::CreateIoCompletionPort(
                handle.as_raw_handle() as *mut _,
                self.inner.handle,
                options.key,
                0,
            );

            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(Handle::new(handle, self.clone(), options.max_buffer_size))
    }

    /// Register the given socket for overlapped I/O and allocate buffers with
    /// the specified capacities that can be used inside of an operation with
    /// it.
    pub fn register_socket<S>(&self, socket: S, options: RegisterOptions) -> io::Result<Socket<S>>
    where
        S: AsRawSocket,
    {
        if options.key >= RESERVED_PORTS {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "trying to use reserved completion port",
            ));
        }

        // Safety: there's nothing inherently unsafe about this.
        unsafe {
            let handle = ioapiset::CreateIoCompletionPort(
                socket.as_raw_socket() as *mut _,
                self.inner.handle,
                options.key,
                0,
            );

            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
        }

        Socket::new(socket, self.clone(), options.max_buffer_size)
    }

    /// Post a message to the I/O completion port.
    pub fn post(&self, completion_port: usize, overlapped: *mut ()) -> io::Result<()> {
        unsafe {
            let result = ioapiset::PostQueuedCompletionStatus(
                self.inner.handle,
                0,
                completion_port,
                overlapped as *mut _,
            );

            if result == FALSE {
                return Err(io::Error::last_os_error());
            }

            Ok(())
        }
    }

    /// Dequeue the next I/O completion status, driving the completion port in
    /// the process.
    ///
    /// Returns [CompletionPoll::Shutdown] if
    /// [shutdown][CompletionPort::shutdown] has been called.
    pub fn poll(&self) -> io::Result<CompletionPoll> {
        trace!("poll");

        let status = self.get_queued_completion_status()?;

        if status.completion_key >= RESERVED_PORTS {
            match status.completion_key {
                SHUTDOWN_PORT => {
                    let pending = self.inner.pending.fetch_add(isize::MIN, Ordering::SeqCst);
                    trace!(status = ?status, pending = pending, "shutdown");
                    debug_assert!(pending >= 0);
                    let pending = pending as usize;
                    return Ok(CompletionPoll::Shutdown(Shutdown { pending }));
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "got status for reserved completion port",
                    ))
                }
            }
        }

        let _pending = self.inner.pending.fetch_sub(1, Ordering::AcqRel);
        trace!(status = ?status, pending = _pending, "completion");

        Ok(CompletionPoll::Status(status))
    }

    /// Dequeue the next I/O completion status while shutting down, driving the
    /// completion port in the process to consume all pending events.
    pub fn poll_during_shutdown(&self) -> io::Result<CompletionStatus> {
        loop {
            trace!("poll_during_shutdown");

            let status = self.get_queued_completion_status()?;

            trace! {
                state = "wakeup",
                status = ?status,
                "poll_during_shutdown",
            };

            if status.completion_key >= RESERVED_PORTS {
                match status.completion_key {
                    // shutdown called again.
                    SHUTDOWN_PORT => continue,
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            "got status for reserved completion port",
                        ))
                    }
                }
            }

            return Ok(status);
        }
    }

    /// Shut down the current completion port. This will cause
    /// [poll][CompletionPort::poll] to return
    /// [CompletionPoll::Shutdown][crate::CompletionPoll::Shutdown].
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> std::io::Result<()> {
    /// let (port, handle) = iocp::setup(1)?;
    /// port.shutdown()?;
    /// handle.join()?;
    /// # Ok(()) }
    /// ```
    pub fn shutdown(&self) -> io::Result<()> {
        self.post(SHUTDOWN_PORT, ptr::null_mut())
    }

    fn get_queued_completion_status(&self) -> io::Result<CompletionStatus> {
        // Safety: this is handled internally of this crate.
        unsafe {
            let mut bytes_transferred = mem::MaybeUninit::zeroed();
            let mut completion_key = mem::MaybeUninit::zeroed();
            let mut overlapped = mem::MaybeUninit::zeroed();

            let result = ioapiset::GetQueuedCompletionStatus(
                self.inner.handle,
                bytes_transferred.as_mut_ptr(),
                completion_key.as_mut_ptr(),
                overlapped.as_mut_ptr(),
                winbase::INFINITE,
            );

            trace!(result = result, "get_queued_completion_status");

            let outcome = if result == FALSE {
                match errhandlingapi::GetLastError() {
                    winerror::ERROR_OPERATION_ABORTED => CompletionOutcome::Aborted,
                    _ => CompletionOutcome::Errored,
                }
            } else {
                CompletionOutcome::Completed
            };

            let bytes_transferred = bytes_transferred.assume_init();
            let completion_key = completion_key.assume_init();
            let overlapped = overlapped.assume_init();

            let header = if !overlapped.is_null() {
                Some(Header::from_overlapped(overlapped))
            } else {
                None
            };

            Ok(CompletionStatus {
                header,
                completion_key,
                bytes_transferred,
                outcome,
            })
        }
    }
}

unsafe impl Send for CompletionPort {}
unsafe impl Sync for CompletionPort {}

impl fmt::Debug for CompletionPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionPort")
            .field("handle", &self.inner.handle)
            .finish()
    }
}

/// The outcome of a completion.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum CompletionOutcome {
    /// The operation was completed.
    Completed,
    /// The operation was aborted.
    Aborted,
    /// The task errored.
    Errored,
}

/// The status of a single completion.
#[derive(Debug)]
#[non_exhaustive]
pub struct CompletionStatus {
    /// The header associated with the I/O operation.
    pub(crate) header: Option<Arc<Header>>,
    /// The completion key woken up.
    pub completion_key: usize,
    /// The number of bytes transferred.
    pub bytes_transferred: u32,
    /// The outcome of the operation.
    pub outcome: CompletionOutcome,
}

impl CompletionStatus {
    /// Unlock the I/O resources associated with this completion status.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use iocp::{CompletionPort, CompletionPoll};
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let port = CompletionPort::create(2)?;
    ///
    /// let pending = loop {
    ///     match port.poll()? {
    ///         CompletionPoll::Status(status) => {
    ///             status.unlock();
    ///             port.shutdown();
    ///         }
    ///         CompletionPoll::Shutdown(shutdown) => {
    ///             break shutdown.pending;
    ///         }
    ///         _ => unreachable!(),
    ///     }
    /// };
    ///
    /// assert_eq!(pending, 0);
    /// # Ok(()) }
    /// ```
    pub fn unlock(&self) {
        if let Some(header) = &self.header {
            header.unlock();
        }
    }

    /// Release the task associated with this completion status.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use iocp::{CompletionPort, CompletionPoll};
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let port = CompletionPort::create(2)?;
    ///
    /// let pending = loop {
    ///     match port.poll()? {
    ///         CompletionPoll::Status(status) => {
    ///             status.unlock();
    ///             port.shutdown();
    ///         }
    ///         CompletionPoll::Shutdown(shutdown) => {
    ///             break shutdown.pending;
    ///         }
    ///         _ => unreachable!(),
    ///     }
    /// };
    ///
    /// assert_eq!(pending, 0);
    /// # Ok(()) }
    /// ```
    pub fn release(&self) {
        if let Some(header) = &self.header {
            header.release()
        }
    }
}

/// Information associated with the completion port is shutting down.
#[non_exhaustive]
pub struct Shutdown {
    /// The number of pending I/O operations at the time of shutting down.
    pub pending: usize,
}

/// The result of polling the completion port through
/// [poll][CompletionPort::poll].
#[non_exhaustive]
pub enum CompletionPoll {
    /// Port is shutting down and there's the given number of pending operations
    /// to contend with.
    Shutdown(Shutdown),
    /// A returned completion status.
    Status(CompletionStatus),
}

pub struct CompletionPortPermit<'a> {
    pub(crate) port: &'a CompletionPort,
}

impl Drop for CompletionPortPermit<'_> {
    fn drop(&mut self) {
        let _pending = self.port.inner.pending.fetch_sub(1, Ordering::SeqCst);
        trace!(pending = _pending, "releasing permit from guard");
    }
}

/// The port to post a message for when we are ready to shut down.
const SHUTDOWN_PORT: usize = usize::MAX;
/// Any ports equal or larger than this value is reserved.
const RESERVED_PORTS: usize = !0b1111;

/// The shared interior of the completion port.
struct Inner {
    /// The number of pending I/O operations, where it's expected how many
    /// [Handle]'s have submitted an operation to be pending.
    ///
    /// Is negative if the port is shutting down.
    pending: AtomicIsize,
    /// The inner handle.
    handle: HANDLE,
}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe {
            // NB: intentionally ignored.
            let _ = handleapi::CloseHandle(self.handle);
        }
    }
}
