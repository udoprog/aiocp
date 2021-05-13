use std::fmt;
use std::os::windows::io::{AsRawHandle, RawHandle};
use winapi::um::handleapi;
use winapi::um::winnt::HANDLE;

/// A raw handle.
pub struct Handle {
    handle: HANDLE,
}

impl Handle {
    /// Construct from a raw handle.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the Handle is solely owned
    /// by this structure after calling this function and that it doesn't
    /// reference a resource that is not thread-safe ([Send] + [Sync]).
    pub unsafe fn from_raw(handle: HANDLE) -> Self {
        Self { handle }
    }

    /// Construct a handle around an invalid handle value.
    pub fn invalid() -> Self {
        Self {
            handle: handleapi::INVALID_HANDLE_VALUE,
        }
    }
}

impl AsRawHandle for Handle {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle as *mut _
    }
}

unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field("handle", &self.handle)
            .finish()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            // intentionally ignored.
            let _ = handleapi::CloseHandle(self.handle);
        }
    }
}
