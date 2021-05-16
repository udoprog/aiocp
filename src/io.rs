use crate::task::Header;
use winapi::um::minwinbase;

/// The internal state of the driver. This can be fully determined by the driver
/// during locking.
#[derive(Debug, Clone, Copy)]
pub enum OverlappedState {
    /// The operation is idle, and the local task owns all resources associated
    /// with it.
    Idle,
    /// An operation is pending on the remote completion port. The remote
    /// completion port owns all resources associated with it.
    Pending,
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
