use crate::handle::Handle;
use crate::sys::AsRawHandle;
use crate::task::Header;
use std::fmt;
use std::io;
use std::sync::Arc;

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
    imp: crate::sys::CompletionPort,
}

impl CompletionPort {
    /// Create a new completion port.
    pub fn create(threads: u32) -> io::Result<Self> {
        Ok(Self {
            imp: crate::sys::CompletionPort::create(threads)?,
        })
    }

    /// Acquire a permit from the completion port with the intention of
    /// performing I/O operations over it. The returned handle will release it
    /// from I/O unless it is forgotten.
    pub fn permit(&self) -> io::Result<CompletionPortPermit<'_>> {
        Ok(CompletionPortPermit {
            _imp: self.imp.permit()?,
        })
    }

    /// Post a message to the I/O completion port.
    pub fn post(&self, completion_port: usize, overlapped: *mut ()) -> io::Result<()> {
        self.imp.post(completion_port, overlapped)
    }

    /// Dequeue the next I/O completion status, driving the completion port in
    /// the process.
    ///
    /// Returns [CompletionPoll::Shutdown] if
    /// [shutdown][CompletionPort::shutdown] has been called.
    pub fn poll(&self) -> io::Result<CompletionPoll> {
        self.imp.poll()
    }

    /// Dequeue the next I/O completion status while shutting down, driving the
    /// completion port in the process to consume all pending events.
    pub fn poll_during_shutdown(&self) -> io::Result<CompletionStatus> {
        self.imp.poll_during_shutdown()
    }

    /// Register the given handle for overlapped I/O and allocate buffers with
    /// the specified capacities that can be used inside of an operation with
    /// it.
    pub fn register_handle<H>(&self, handle: H, options: RegisterOptions) -> io::Result<Handle<H>>
    where
        H: AsRawHandle,
    {
        self.imp
            .register_handle(handle, options.key, options.max_buffer_size)
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
        self.imp.shutdown()
    }
}

impl fmt::Debug for CompletionPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.imp.fmt(f)
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
    _imp: crate::sys::CompletionPortPermit<'a>,
}
