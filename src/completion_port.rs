use crate::handle::Handle;
use crate::io::IocpOverlappedHeader;
use crate::iocp_handle::IocpHandle;
use std::fmt;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::ptr;
use std::sync::Arc;
use winapi::shared::minwindef::FALSE;
use winapi::um::handleapi;
use winapi::um::ioapiset;
use winapi::um::winbase;
use winapi::um::winnt::HANDLE;

/// The port to post a message for when we are ready to shut down.
const SHUTDOWN_PORT: usize = usize::MAX;
/// Any ports equal or larger than this value is reserved.
const RESERVED_PORTS: usize = !0b1111;

/// The asynchronous handler for a Windows I/O Completion Port.
///
/// Permits the user to use it and wait for an overlapped result to complete.
pub struct CompletionPort {
    handle: Handle,
}

unsafe impl Send for CompletionPort {}
unsafe impl Sync for CompletionPort {}

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
                handle: Handle::from_raw(handle),
            })
        }
    }

    /// Post a message to the I/O completion port.
    pub fn post(&self, completion_port: usize, overlapped: *mut ()) -> io::Result<()> {
        unsafe {
            let result = ioapiset::PostQueuedCompletionStatus(
                self.handle.as_raw_handle() as *mut _,
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
    /// Returns `None` if [shutdown] has been called.
    pub fn get_queued_completion_status(&self) -> io::Result<Option<CompletionStatus>> {
        unsafe {
            let mut bytes_transferred = mem::MaybeUninit::zeroed();
            let mut completion_key = mem::MaybeUninit::zeroed();
            let mut overlapped = mem::MaybeUninit::uninit();

            let result = ioapiset::GetQueuedCompletionStatus(
                self.handle.as_raw_handle() as *mut _,
                bytes_transferred.as_mut_ptr(),
                completion_key.as_mut_ptr(),
                overlapped.as_mut_ptr(),
                winbase::INFINITE,
            );

            if result == FALSE {
                return Err(io::Error::last_os_error());
            }

            let bytes_transferred = bytes_transferred.assume_init();
            let completion_key = completion_key.assume_init();
            let overlapped = overlapped.assume_init();

            if completion_key == SHUTDOWN_PORT {
                return Ok(None);
            }

            let header = IocpOverlappedHeader::from_overlapped(overlapped);

            Ok(Some(CompletionStatus {
                header,
                completion_key,
                bytes_transferred,
            }))
        }
    }

    /// Register the given handle for overlapped I/O and allocate buffers with
    /// the specified capacities that can be used inside of an operation with
    /// it.
    pub fn register<H>(&self, handle: H, key: usize) -> io::Result<IocpHandle<H>>
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
        Ok(IocpHandle::new(handle))
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
            ioapiset::CreateIoCompletionPort(handle, self.handle.as_raw_handle() as *mut _, key, 0)
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
            .field("handle", &self.handle)
            .finish()
    }
}

/// The status of a single completion.
#[non_exhaustive]
pub struct CompletionStatus {
    /// The header associated with the I/O operation.
    header: Arc<IocpOverlappedHeader>,
    /// The completion key woken up.
    pub completion_key: usize,
    /// The number of bytes transferred.
    pub bytes_transferred: u32,
}

impl CompletionStatus {
    /// Release the task associated with this completion status.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let port = Arc::new(CompletionPort::create(2)?);
    ///
    /// loop {
    ///     let status = port.get_queued_completion_status().expect("failed to get next status");
    ///     status.release();
    /// }
    /// # Ok(()) }
    /// ```
    pub fn release(&self) {
        self.header.release()
    }
}
