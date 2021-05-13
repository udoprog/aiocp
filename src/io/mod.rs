use crate::atomic_waker::AtomicWaker;
use crate::pool::IocpPool;
use std::cell::UnsafeCell;
use std::mem;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use winapi::um::minwinbase;

mod operation;
pub use self::operation::Operation;

/// The results of an overlapped operation.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct OverlappedResult {
    pub bytes_transferred: usize,
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
pub(crate) struct IocpOverlappedHeader {
    pub(crate) raw: UnsafeCell<minwinbase::OVERLAPPED>,
    pool: UnsafeCell<IocpPool>,
    lock: AtomicIsize,
    pub(crate) waker: AtomicWaker,
}

impl IocpOverlappedHeader {
    pub(crate) fn new() -> Self {
        IocpOverlappedHeader {
            // Safety: OVERLAPPED structure is valid when zeroed.
            raw: unsafe { mem::MaybeUninit::zeroed().assume_init() },
            pool: UnsafeCell::new(IocpPool::new()),
            lock: AtomicIsize::new(0),
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
        self.unlock();
        self.wake();
    }

    /// Try to lock the overlap and return the pointer to the associated
    /// overlapped struct.
    pub(crate) fn lock<'a>(
        self: &'a Arc<Self>,
    ) -> Option<(*mut minwinbase::OVERLAPPED, Guard<'a>)> {
        let n = self.lock.fetch_add(1, Ordering::SeqCst);

        if n != 0 {
            self.lock.fetch_sub(1, Ordering::SeqCst);
            return None;
        }

        let overlapped = Arc::into_raw(self.clone()) as *const minwinbase::OVERLAPPED as *mut _;

        Some((overlapped, Guard { header: self }))
    }

    /// Unlock access to resources associated with this operation.
    #[inline]
    fn unlock(&self) {
        // Safety: This is only accessible at positions in the program where
        // resources are no longer in use by the operating system, so this
        // cannot be incorrectly used.
        self.lock.fetch_sub(1, Ordering::SeqCst);
    }

    /// Wake the async task waiting for this I/O to complete.
    #[inline]
    fn wake(&self) {
        self.waker.wake();
    }
}

impl Drop for IocpOverlappedHeader {
    fn drop(&mut self) {
        unsafe { (*self.pool.get()).drop_in_place() }
    }
}

/// A lock guard that will unlock the resource if dropped.
///
/// Dropping the guard automatically clears the available buffers and unlocks
/// the header.
pub(crate) struct Guard<'a> {
    header: &'a IocpOverlappedHeader,
}

impl Guard<'_> {
    /// Access the pool associated with the locked state of the header.
    pub(crate) fn pool(&mut self) -> &mut IocpPool {
        unsafe { &mut *self.header.pool.get() }
    }

    /// Forget the guard, causing it to not unlock on drop.
    pub(crate) fn forget(self) {
        let _ = mem::ManuallyDrop::new(self);
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.pool().reset();
        self.header.unlock();
    }
}
