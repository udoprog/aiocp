use crate::atomic_waker::AtomicWaker;
use crate::ops;
use crate::pool::BufferPool;
use std::cell::UnsafeCell;
use std::io;
use std::mem;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use winapi::um::minwinbase;

/// Helper to deal with I/O pending errors correctly.
///
/// Returns Some(e) if an immediate error should be returned.
pub(crate) fn handle_io_pending<O>(result: io::Result<O>) -> io::Result<Option<O>> {
    Ok(match result {
        Ok(output) => Some(output),
        Err(e) if e.raw_os_error() == Some(crate::errors::ERROR_IO_PENDING) => None,
        Err(e) => return Err(e),
    })
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
            let _ = OverlappedHeader::from_overlapped(self.raw);
        }
    }
}

#[repr(C)]
pub(crate) struct OverlappedHeader {
    pub(crate) raw: UnsafeCell<minwinbase::OVERLAPPED>,
    pool: BufferPool,
    lock: AtomicI64,
    pub(crate) waker: AtomicWaker,
}

impl OverlappedHeader {
    pub(crate) fn new() -> Self {
        OverlappedHeader {
            // Safety: OVERLAPPED structure is valid when zeroed.
            raw: unsafe { mem::MaybeUninit::zeroed().assume_init() },
            pool: BufferPool::new(),
            lock: AtomicI64::new(0),
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
    pub(crate) fn lock<'a>(
        self: &'a Arc<Self>,
        ops::Code(id): ops::Code,
    ) -> Option<OverlappedGuard<'a>> {
        let id = id as i64 + 1; // to ensure that we never use 0

        while let Err(n) =
            self.lock
                .compare_exchange_weak(0, id, Ordering::SeqCst, Ordering::Relaxed)
        {
            // Already locked, either by our kind of operation or someone else.
            if n > 0 || id != -n {
                return None;
            }

            // Attempt to unlock something specifically locked for us.
            let op = self
                .lock
                .compare_exchange_weak(n, id, Ordering::SeqCst, Ordering::Relaxed);

            if op.is_ok() {
                break;
            }
        }

        Some(OverlappedGuard { header: self })
    }

    /// Unlock access to resources associated with this operation.
    #[inline]
    pub fn unlock(&self) -> bool {
        let mut old = self.lock.load(Ordering::Relaxed);

        while old > 0 {
            if let Err(o) =
                self.lock
                    .compare_exchange_weak(old, -old, Ordering::SeqCst, Ordering::Relaxed)
            {
                old = o;
                continue;
            }

            return true;
        }

        false
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
    /// Clear and return the current pool.
    pub fn clear_and_get_pool(&self) -> &BufferPool {
        let pool = self.pool();
        pool.clear();
        pool
    }

    /// Access the pool associated with the locked state of the header.
    pub fn pool(&self) -> &BufferPool {
        &self.header.pool
    }

    /// Get the current header as an overlapped pointer.
    pub fn overlapped(&self) -> Overlapped {
        Overlapped::from_raw(
            Arc::into_raw(self.header.clone()) as *const minwinbase::OVERLAPPED as *mut _,
        )
    }
}

impl Drop for OverlappedGuard<'_> {
    fn drop(&mut self) {
        self.pool().clear();
        self.header.unlock();
    }
}
