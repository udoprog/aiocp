use std::os::windows::io::{AsRawHandle, RawHandle};
use winapi::um::handleapi;
use winapi::um::winnt::HANDLE;

/// A raw handle.
pub struct Handle {
    handle: HANDLE,
}

impl Handle {
    /// Construct from a raw handle.
    pub unsafe fn from_raw(handle: HANDLE) -> Self {
        Self { handle }
    }
}

impl AsRawHandle for Handle {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle as *mut _
    }
}

unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            // intentionally ignored.
            let _ = handleapi::CloseHandle(self.handle);
        }
    }
}
