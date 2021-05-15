use crate::atomic_waker::AtomicWaker;
use crate::pool::BufferPool;
use std::cell::UnsafeCell;
use std::io;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use winapi::um::minwinbase;

/// Helper to deal with I/O pending errors correctly.
///
/// Returns Some(e) if an immediate error should be returned.
pub(crate) fn handle_io_pending<O>(
    result: io::Result<O>,
    permit: crate::sys::CompletionPortPermit<'_>,
    guard: OverlappedGuard<'_>,
) -> Option<io::Error> {
    match result {
        Ok(..) => (),
        Err(e) if e.raw_os_error() == Some(crate::errors::ERROR_IO_PENDING) => (),
        Err(e) => return Some(e),
    }

    std::mem::forget((permit, guard));
    None
}

/// An overlapped structure.
#[derive(Debug)]
pub struct Overlapped {
    ptr: *mut minwinbase::OVERLAPPED,
}

impl Overlapped {
    /// Convert from a raw pointer.
    pub(crate) fn from_raw(ptr: *mut minwinbase::OVERLAPPED) -> Self {
        Self { ptr }
    }

    /// Access a pointer to the underlying overlapped struct.
    pub(crate) fn as_ptr(&mut self) -> *mut minwinbase::OVERLAPPED {
        self.ptr
    }
}

#[repr(C)]
pub(crate) struct OverlappedHeader {
    pub(crate) raw: UnsafeCell<minwinbase::OVERLAPPED>,
    pool: UnsafeCell<BufferPool>,
    lock: AtomicBool,
    pub(crate) waker: AtomicWaker,
}

impl OverlappedHeader {
    pub(crate) fn new() -> Self {
        OverlappedHeader {
            // Safety: OVERLAPPED structure is valid when zeroed.
            raw: unsafe { mem::MaybeUninit::zeroed().assume_init() },
            pool: UnsafeCell::new(BufferPool::new()),
            lock: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        }
    }

    /// Construct from the given pointer to an overlapped object.
    ///
    /// # Safety
    ///
    /// This must only ever be used through overlapped objects provided through
    /// this library, because they expect the OVERLAPPED object to be a pointer
    /// to the first element in this header.
    ///
    /// Coercing *anything* else into this is just wrong.
    pub(crate) unsafe fn from_overlapped(overlapped: *mut minwinbase::OVERLAPPED) -> Arc<Self> {
        Arc::from_raw(overlapped as *const _ as *const Self)
    }

    /// Release the header, allowing a future blocking on it to proceed.
    pub fn release(&self) {
        if self.unlock() {
            self.wake();
        }
    }

    /// Try to lock the overlap and return the pointer to the associated
    /// overlapped struct.
    pub(crate) fn lock<'a>(self: &'a Arc<Self>) -> Option<OverlappedGuard<'a>> {
        if self.lock.swap(true, Ordering::SeqCst) {
            return None;
        }

        Some(OverlappedGuard { header: self })
    }

    /// Unlock access to resources associated with this operation.
    #[inline]
    pub fn unlock(&self) -> bool {
        // Safety: This is only accessible at positions in the program where
        // resources are no longer in use by the operating system, so this
        // cannot be incorrectly used.
        self.lock.swap(false, Ordering::SeqCst)
    }

    /// Wake the async task waiting for this I/O to complete.
    #[inline]
    fn wake(&self) {
        self.waker.wake();
    }
}

/// A lock guard that will unlock the resource if dropped.
///
/// Dropping the guard automatically clears the available buffers and unlocks
/// the header.
pub(crate) struct OverlappedGuard<'a> {
    header: &'a Arc<OverlappedHeader>,
}

impl OverlappedGuard<'_> {
    /// Access the pool associated with the locked state of the header.
    pub(crate) fn pool(&mut self) -> &mut BufferPool {
        unsafe { &mut *self.header.pool.get() }
    }

    /// Get the current header as an overlapped pointer.
    pub(crate) fn overlapped(&self) -> Overlapped {
        Overlapped::from_raw(
            Arc::into_raw(self.header.clone()) as *const minwinbase::OVERLAPPED as *mut _,
        )
    }
}

impl Drop for OverlappedGuard<'_> {
    fn drop(&mut self) {
        self.pool().reset();
        self.header.unlock();
    }
}
