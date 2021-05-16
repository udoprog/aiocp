use crate::completion_port::{CompletionOutcome, CompletionPoll, CompletionStatus, Shutdown};
use crate::handle::Handle;
use crate::io::OverlappedHeader;
use crate::overlapped_handle::OverlappedHandle;
use std::fmt;
use std::io;
use std::mem;
pub use std::os::windows::io::AsRawHandle;
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

#[derive(Clone)]
pub(crate) struct CompletionPort {
    inner: Arc<Inner>,
}

impl CompletionPort {
    pub(crate) fn create(threads: u32) -> io::Result<Self> {
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

    pub(crate) fn permit(&self) -> io::Result<CompletionPortPermit<'_>> {
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

    pub(crate) fn register<H>(&self, handle: H, key: usize) -> io::Result<OverlappedHandle<H>>
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

    /// Post a message to the I/O completion port.
    pub(crate) fn post(&self, completion_port: usize, overlapped: *mut ()) -> io::Result<()> {
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

    pub(crate) fn poll(&self) -> io::Result<CompletionPoll> {
        trace!("poll");

        let (bytes_transferred, completion_key, overlapped, outcome) =
            self.get_queued_completion_status()?;

        if completion_key >= RESERVED_PORTS {
            match completion_key {
                SHUTDOWN_PORT => {
                    let pending = self.inner.pending.fetch_add(isize::MIN, Ordering::SeqCst);

                    trace! {
                        completion_key = completion_key,
                        overlapped = ?overlapped,
                        outcome = ?outcome,
                        pending = pending,
                        "shutdown",
                    };

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

        trace! {
            completion_key = completion_key,
            overlapped = ?overlapped,
            outcome = ?outcome,
            pending = _pending,
            "completion",
        };

        // Safety: this is handled internally of this crate.
        let header = unsafe { OverlappedHeader::from_overlapped(overlapped) };

        Ok(CompletionPoll::Status(CompletionStatus {
            header,
            completion_key,
            bytes_transferred,
            outcome,
        }))
    }

    pub(crate) fn poll_during_shutdown(&self) -> io::Result<CompletionStatus> {
        loop {
            trace!("poll_during_shutdown");

            let (bytes_transferred, completion_key, overlapped, outcome) =
                self.get_queued_completion_status()?;

            trace! {
                state = "wakeup",
                overlapped = ?overlapped,
                "poll_during_shutdown",
            };

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

    pub(crate) fn shutdown(&self) -> io::Result<()> {
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

            Ok((bytes_transferred, completion_key, overlapped, outcome))
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

pub(crate) struct CompletionPortPermit<'a> {
    port: &'a CompletionPort,
}

impl Drop for CompletionPortPermit<'_> {
    fn drop(&mut self) {
        let _pending = self.port.inner.pending.fetch_sub(1, Ordering::SeqCst);
        trace!(pending = _pending, "releasing permit from guard");
    }
}
