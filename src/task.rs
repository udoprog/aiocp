use crate::atomic_waker::AtomicWaker;
use crate::io::{Overlapped, OverlappedState};
use crate::ops;
use crate::pool::BufferPool;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::task::Waker;
use winapi::um::minwinbase;

#[repr(C)]
pub(crate) struct Header {
    raw: UnsafeCell<minwinbase::OVERLAPPED>,
    pool: BufferPool,
    lock: AtomicI64,
    waker: AtomicWaker,
}

impl Header {
    pub(crate) fn new() -> Self {
        Header {
            // Safety: OVERLAPPED structure is valid when zeroed.
            raw: unsafe { mem::MaybeUninit::zeroed().assume_init() },
            pool: BufferPool::new(),
            lock: AtomicI64::new(0),
            waker: AtomicWaker::new(),
        }
    }

    /// Register a new waker.
    pub(crate) fn register_by_ref(&self, waker: &Waker) {
        self.waker.register_by_ref(waker);
    }

    /// Raw access to the underlying file handle.
    pub(crate) fn as_raw_overlapped(&self) -> *mut minwinbase::OVERLAPPED {
        self.raw.get()
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
    pub(crate) fn release(&self) {
        if self.unlock() {
            self.wake();
        }
    }

    /// The state of the overlapped guard.
    pub(crate) fn state(&self) -> OverlappedState {
        if self.lock.load(Ordering::SeqCst) <= 0 {
            OverlappedState::Local
        } else {
            OverlappedState::Remote
        }
    }

    /// Try to lock the overlap and return the pointer to the associated
    /// overlapped struct.
    pub(crate) fn lock<'a>(self: &'a Arc<Self>, ops::Code(id): ops::Code) -> LockResult<'a> {
        let id = id as i64 + 1; // to ensure that we never use 0

        let state = loop {
            let op = self
                .lock
                .compare_exchange_weak(0, id, Ordering::SeqCst, Ordering::Relaxed);

            let n = match op {
                Ok(..) => break OverlappedState::Local,
                Err(n) => n,
            };

            // Already locked, either by our kind of operation or someone else.
            if n > 0 {
                return LockResult::Busy(id != -n);
            }

            // Attempt to unlock using our locking code.
            let op = self
                .lock
                .compare_exchange_weak(n, 0, Ordering::SeqCst, Ordering::Relaxed);

            if op.is_ok() {
                break OverlappedState::Remote;
            }
        };

        LockResult::Ok(LockGuard {
            header: self,
            state,
        })
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

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Header")
            .field("overlapped", &self.raw.get())
            .finish()
    }
}

/// A lock guard that will unlock the resource if dropped.
///
/// Dropping the guard automatically clears the available buffers and unlocks
/// the header.
pub(crate) struct LockGuard<'a> {
    header: &'a Arc<Header>,
    state: OverlappedState,
}

impl LockGuard<'_> {
    /// Clear and return the current pool.
    pub fn clear_and_get_pool(&self) -> &BufferPool {
        let pool = self.pool();
        pool.clear();
        pool
    }

    /// Get the state of the guard.
    pub fn state(&self) -> OverlappedState {
        self.state
    }

    /// Access the pool associated with the locked state of the header.
    pub fn pool(&self) -> &BufferPool {
        &self.header.pool
    }

    /// Advance the cursor of the overlapped structure by `amount`.
    ///
    /// This is used by read or write operations to move the read or write
    /// cursor forward.
    pub fn advance(&self, amount: usize) {
        use std::convert::TryFrom as _;

        trace!(amount = amount, "advance");

        // Safety: this is done in the guard, which ensures exclusive access.
        unsafe {
            let overlapped = &mut *self.header.raw.get();
            let s = overlapped.u.s_mut();
            let mut n = s.Offset as u64 | (s.OffsetHigh as u64) << 32;

            n = u64::try_from(amount)
                .ok()
                .and_then(|amount| n.checked_add(amount))
                .expect("advance out of bounds");

            s.Offset = (n & !0u32 as u64) as u32;
            s.OffsetHigh = (n >> 32) as u32;
        }
    }

    /// Get the current header as an overlapped pointer.
    pub fn overlapped(&self) -> Overlapped {
        Overlapped::from_raw(
            Arc::into_raw(self.header.clone()) as *const minwinbase::OVERLAPPED as *mut _,
        )
    }
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        self.pool().clear();
        self.header.unlock();
    }
}

/// The result of trying to lock the overlapped header.
pub(crate) enum LockResult<'a> {
    /// Locking succeeded.
    Ok(LockGuard<'a>),
    /// Locking failed because it's busy. Flag indicates if there was a mismatch
    /// in the type of operation.
    ///
    /// If there was a mismatch, the caller should cancel_if_pending the operation.
    Busy(bool),
}
