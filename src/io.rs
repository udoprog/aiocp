use crate::task::Header;
use std::io;
use winapi::um::minwinbase;

/// A unique code that designates exactly how any one given overlapped result
/// must be treated. This has safety implications, because treating the
/// overlapped results of something like a WRITE as a READ instead could result
/// in assuming that uninitialized memory has been initialized by the write
/// operation.
pub struct Code(pub(crate) u32);

/// The results of an overlapped operation.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct OverlappedResult {
    /// The bytes transferred by this operation. Its exact meaning is determined
    /// by [Operation::collect].
    pub bytes_transferred: usize,
}

impl OverlappedResult {
    /// An empty overlapped result.
    pub(crate) const fn empty() -> Self {
        Self {
            bytes_transferred: 0,
        }
    }
}

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

/// Helper to deal with I/O pending errors correctly.
///
/// Returns Some(e) if an immediate error should be returned.
pub(crate) fn handle_io_pending<O>(result: io::Result<O>) -> io::Result<()> {
    match result {
        Ok(..) => (),
        Err(e) if e.raw_os_error() == Some(crate::errors::ERROR_IO_PENDING) => (),
        Err(e) => return Err(e),
    }

    Ok(())
}
