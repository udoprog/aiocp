use crate::handle::Handle;
use crate::io::OverlappedHeader;
use crate::iocp_handle::OverlappedHandle;
use std::fmt;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use winapi::shared::minwindef::FALSE;
use winapi::shared::winerror;
use winapi::um::errhandlingapi;
use winapi::um::handleapi;
use winapi::um::ioapiset;
use winapi::um::minwinbase;
use winapi::um::winbase;
use winapi::um::winnt::HANDLE;

/// The port to post a message for when we are ready to shut down.
const SHUTDOWN_PORT: usize = usize::MAX;
/// Any ports equal or larger than this value is reserved.
const RESERVED_PORTS: usize = !0b1111;

/// The shared interior of the completion port.
struct Inner {
    /// The number of pending I/O operations, where it's expected how many
    /// [OverlappedHandle]'s have submitted an operation to be pending.
    ///
    /// Is negative if the port is shutting down.
    pending: AtomicIsize,
    /// The inner handle.
    handle: Handle,
}

/// The asynchronous handler for a Windows I/O Completion Port.
///
/// Permits the user to use it and wait for an overlapped result to complete.
#[derive(Clone)]
pub struct CompletionPort {
    inner: Arc<Inner>,
}

unsafe impl Send for CompletionPort {}
unsafe impl Sync for CompletionPort {}

impl CompletionPort {
    /// Acquire a permit from the completion port with the intention of
    /// performing I/O operations over it. The returned handle will release it
    /// from I/O unless it is forgotten using [CompletionPortPermit::forget].
    pub(crate) fn permit(&self) -> io::Result<CompletionPortPermit<'_>> {
        let pending = self.inner.pending.fetch_add(1, Ordering::SeqCst);
        trace!(pending = pending, "acquiring permit");

        if pending >= 0 {
            Ok(CompletionPortPermit { port: self })
        } else {
            self.inner.pending.fetch_sub(1, Ordering::SeqCst);

            Err(io::Error::new(
                io::ErrorKind::Other,
                "port is shutting down",
            ))
        }
    }

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
                    handle: Handle::from_raw(handle),
                }),
            })
        }
    }

    /// Post a message to the I/O completion port.
    pub fn post(&self, completion_port: usize, overlapped: *mut ()) -> io::Result<()> {
        unsafe {
            let result = ioapiset::PostQueuedCompletionStatus(
                self.inner.handle.as_raw_handle() as *mut _,
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

    fn get_queued_completion_status(
        &self,
    ) -> io::Result<(u32, usize, *mut minwinbase::OVERLAPPED, CompletionOutcome)> {
        unsafe {
            let mut bytes_transferred = mem::MaybeUninit::zeroed();
            let mut completion_key = mem::MaybeUninit::zeroed();
            let mut overlapped = mem::MaybeUninit::zeroed();

            let result = ioapiset::GetQueuedCompletionStatus(
                self.inner.handle.as_raw_handle() as *mut _,
                bytes_transferred.as_mut_ptr(),
                completion_key.as_mut_ptr(),
                overlapped.as_mut_ptr(),
                winbase::INFINITE,
            );

            let outcome = if result == FALSE {
                match errhandlingapi::GetLastError() {
                    winerror::ERROR_OPERATION_ABORTED => CompletionOutcome::Cancelled,
                    other => return Err(io::Error::from_raw_os_error(other as i32)),
                }
            } else {
                CompletionOutcome::Ok
            };

            let bytes_transferred = bytes_transferred.assume_init();
            let completion_key = completion_key.assume_init();
            let overlapped = overlapped.assume_init();

            Ok((bytes_transferred, completion_key, overlapped, outcome))
        }
    }

    /// Dequeue the next I/O completion status, driving the completion port in
    /// the process.
    ///
    /// Returns [CompletionPoll::Shutdown] if
    /// [shutdown][CompletionPort::shutdown] has been called.
    pub fn poll(&self) -> io::Result<CompletionPoll> {
        trace!("poll");

        let (bytes_transferred, completion_key, overlapped, outcome) =
            self.get_queued_completion_status()?;

        if completion_key >= RESERVED_PORTS {
            match completion_key {
                SHUTDOWN_PORT => {
                    let pending =
                        self.inner.pending.fetch_add(isize::MIN, Ordering::SeqCst) as usize;
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
        trace!(pending = _pending, "releasing permit during poll");

        // Safety: this is handled internally of this crate.
        let header = unsafe { OverlappedHeader::from_overlapped(overlapped) };

        Ok(CompletionPoll::Status(CompletionStatus {
            header,
            completion_key,
            bytes_transferred,
            outcome,
        }))
    }

    /// Dequeue the next I/O completion status while shutting down, driving the
    /// completion port in the process to consume all pending events.
    pub fn poll_during_shutdown(&self) -> io::Result<CompletionStatus> {
        loop {
            trace!("poll_during_shutdown");

            let (bytes_transferred, completion_key, overlapped, outcome) =
                self.get_queued_completion_status()?;

            if completion_key >= RESERVED_PORTS {
                match completion_key {
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

            // Safety: this is handled internally of this crate.
            let header = unsafe { OverlappedHeader::from_overlapped(overlapped) };

            return Ok(CompletionStatus {
                header,
                completion_key,
                bytes_transferred,
                outcome,
            });
        }
    }

    /// Register the given handle for overlapped I/O and allocate buffers with
    /// the specified capacities that can be used inside of an operation with
    /// it.
    pub fn register<H>(&self, handle: H, key: usize) -> io::Result<OverlappedHandle<H>>
    where
        H: AsRawHandle,
    {
        if key >= RESERVED_PORTS {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "trying to use reserved completion port",
            ));
        }

        self.register_handle(handle.as_raw_handle() as *mut _, key)?;
        Ok(OverlappedHandle::new(handle, self.clone()))
    }

    /// Shut down the current completion port. This will cause
    /// [get_queued_completion_status][CompletionPort::get_queued_completion_status]
    /// to immediately return `None` forever.
    pub fn shutdown(&self) -> io::Result<()> {
        self.post(SHUTDOWN_PORT, ptr::null_mut())
    }

    /// Associate a new handle with the I/O completion port.
    ///
    /// This means any overlapped operations associated with the given handle
    /// will notify this completion port.
    fn register_handle(&self, handle: HANDLE, key: usize) -> io::Result<()> {
        let handle = unsafe {
            ioapiset::CreateIoCompletionPort(
                handle,
                self.inner.handle.as_raw_handle() as *mut _,
                key,
                0,
            )
        };

        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }
}

impl fmt::Debug for CompletionPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionPort")
            .field("handle", &self.inner.handle)
            .finish()
    }
}

/// The outcome of a completion.
#[non_exhaustive]
pub enum CompletionOutcome {
    /// The operation was completed successfully.
    Ok,
    /// The operation was cancelled.
    Cancelled,
}

/// The status of a single completion.
#[non_exhaustive]
pub struct CompletionStatus {
    /// The header associated with the I/O operation.
    header: Arc<OverlappedHeader>,
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
    /// use aiocp::{CompletionPort, CompletionPoll};
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
        self.header.unlock();
    }

    /// Release the task associated with this completion status.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use aiocp::{CompletionPort, CompletionPoll};
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
        self.header.release()
    }
}

/// Information associated with the completion port is shutting down.
#[non_exhaustive]
pub struct Shutdown {
    /// The number of pending I/O operations at the time of shutting down.
    pub pending: usize,
}

/// The result of polling the completion port through
/// [get_queued_completion_status][CompletionPort::get_queued_completion_status].
#[non_exhaustive]
pub enum CompletionPoll {
    /// Port is shutting down and there's the given number of pending operations
    /// to contend with.
    Shutdown(Shutdown),
    /// A returned completion status.
    Status(CompletionStatus),
}

pub struct CompletionPortPermit<'a> {
    port: &'a CompletionPort,
}

impl CompletionPortPermit<'_> {
    /// Forget the current I/O permit, under the assumption that it has been
    /// taken over by a [CompletionPort].
    pub fn forget(self) {
        let _ = mem::ManuallyDrop::new(self);
    }
}

impl Drop for CompletionPortPermit<'_> {
    fn drop(&mut self) {
        let _pending = self.port.inner.pending.fetch_sub(1, Ordering::SeqCst);
        trace!(pending = _pending, "releasing permit");
    }
}
