use crate::task::Header;
use winapi::um::minwinbase;

/// The internal state of the driver. This can be fully determined by the driver
/// during locking.
#[derive(Debug, Clone, Copy)]
pub enum OverlappedState {
    /// The driver currently owns the operation. The critical section associated
    /// with the operation is owned by the driver.
    Local,
    /// The remote completion port has marked that an operation is completed.
    Complete,
}

/// An overlapped structure.
#[derive(Debug)]
pub struct Overlapped {
    raw: *mut minwinbase::OVERLAPPED,
}

impl Overlapped {
    /// Convert from a raw pointer.
    pub(crate) fn from_raw(raw: *mut minwinbase::OVERLAPPED) -> Self {
        Self { raw }
    }

    /// Access a pointer to the underlying overlapped struct.
    pub(crate) fn as_ptr(&mut self) -> *mut minwinbase::OVERLAPPED {
        self.raw
    }
}

impl Drop for Overlapped {
    fn drop(&mut self) {
        unsafe {
            // NB: by default we make sure to consume the header that this
            // pointer points towards.
            //
            // It is up to the user of this overlapped structure to forget it
            // otherwise.
            let _ = Header::from_overlapped(self.raw);
        }
    }
}
