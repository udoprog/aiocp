use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::ptr;
use winapi::shared::minwindef::FALSE;
use winapi::um::handleapi;
use winapi::um::ioapiset;
use winapi::um::winbase;
use winapi::um::winnt::HANDLE;

mod atomic_waker;

pub mod flags;

mod write_ext;
pub use self::write_ext::WriteExt;

mod overlapped;
use self::overlapped::OverlappedHeader;
pub use self::overlapped::{Overlapped, OverlappedResult, RawOverlapped};

mod buf;
pub use self::buf::{Pool, RawBuf};

/// The status of a single completion.
pub struct CompletionStatus {
    /// The completion key woken up.
    pub completion_key: usize,
    /// The number of bytes transferred.
    pub bytes_transferred: u32,
}

/// The asynchronous handler for a Windows I/O Completion Port.
///
/// Permits the user to use it and wait for an overlapped result to complete.
pub struct IoCompletionPort {
    handle: HANDLE,
}

unsafe impl Send for IoCompletionPort {}
unsafe impl Sync for IoCompletionPort {}

impl IoCompletionPort {
    /// Create a new completion port.
    pub fn create(threads: u32) -> io::Result<Self> {
        let handle = unsafe {
            ioapiset::CreateIoCompletionPort(
                handleapi::INVALID_HANDLE_VALUE,
                ptr::null_mut(),
                0,
                threads,
            )
        };

        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }

        Ok(Self { handle })
    }

    /// Dequeue the next I/O completion status.
    pub fn wait(&self) -> io::Result<CompletionStatus> {
        unsafe {
            loop {
                let mut bytes_transferred = mem::MaybeUninit::zeroed();
                let mut completion_key = mem::MaybeUninit::zeroed();
                let mut overlapped = mem::MaybeUninit::uninit();

                let result = ioapiset::GetQueuedCompletionStatus(
                    self.handle,
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

                let header = OverlappedHeader::from_overlapped(overlapped as *const _);
                header.unlock();
                header.wake();

                return Ok(CompletionStatus {
                    completion_key,
                    bytes_transferred,
                });
            }
        }
    }

    /// Associate a new handle with the I/O completion port.
    ///
    /// This means any overlapped operations associated with the given handle
    /// will notify this completion port.
    pub fn register_handle<H>(&self, handle: &H) -> io::Result<()>
    where
        H: AsRawHandle,
    {
        self._register_handle(handle.as_raw_handle())
    }

    fn _register_handle(&self, handle: HANDLE) -> io::Result<()> {
        let handle = unsafe { ioapiset::CreateIoCompletionPort(handle, self.handle, 0, 0) };

        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }
}

impl Drop for IoCompletionPort {
    fn drop(&mut self) {
        unsafe {
            // intentionally ignored.
            let _ = handleapi::CloseHandle(self.handle);
        }
    }
}
