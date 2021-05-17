use crate::completion_port::{CompletionOutcome, CompletionPoll, CompletionStatus, Shutdown};
use crate::handle::Handle;
use crate::socket::Socket;
use crate::task::Header;
use std::fmt;
use std::io;
use std::mem;
pub use std::os::windows::io::{AsRawHandle, AsRawSocket, RawHandle, RawSocket};
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
                    handle,
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

    /// Associate a new handle with the I/O completion port.
    ///
    /// This means any overlapped operations associated with the given handle
    /// will notify this completion port.
    pub(crate) fn register_handle<H>(
        &self,
        handle: H,
        key: usize,
        max_buffer_size: usize,
    ) -> io::Result<Handle<H>>
    where
        H: AsRawHandle,
    {
        if key >= RESERVED_PORTS {
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
                key,
                0,
            );

            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(Handle::new(handle, self.clone(), max_buffer_size))
    }

    /// Associate a new socket with the I/O completion port.
    ///
    /// This means any overlapped operations associated with the given handle
    /// will notify this completion port.
    pub(crate) fn register_socket<S>(
        &self,
        socket: S,
        key: usize,
        max_buffer_size: usize,
    ) -> io::Result<Socket<S>>
    where
        S: AsRawSocket,
    {
        if key >= RESERVED_PORTS {
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
                key,
                0,
            );

            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(Socket::new(socket, self.clone(), max_buffer_size))
    }

    /// Post a message to the I/O completion port.
    pub(crate) fn post(&self, completion_port: usize, overlapped: *mut ()) -> io::Result<()> {
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

    pub(crate) fn poll(&self) -> io::Result<CompletionPoll> {
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

    pub(crate) fn poll_during_shutdown(&self) -> io::Result<CompletionStatus> {
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

    pub(crate) fn shutdown(&self) -> io::Result<()> {
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

pub(crate) struct CompletionPortPermit<'a> {
    port: &'a CompletionPort,
}

impl Drop for CompletionPortPermit<'_> {
    fn drop(&mut self) {
        let _pending = self.port.inner.pending.fetch_sub(1, Ordering::SeqCst);
        trace!(pending = _pending, "releasing permit from guard");
    }
}
