use crate::atomic_waker::AtomicWaker;
use crate::io::{Code, OverlappedState};
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
    pub(crate) raw: UnsafeCell<minwinbase::OVERLAPPED>,
    pub(crate) pool: BufferPool,
    /// Lock for the current overlapped operation.
    ///
    /// This can have one of three states:
    ///
    /// * `0`   -   Which is the open state.
    /// * `n > 0` - Which indicates that an overlapped task with a corresponding
    ///             [Code] has been submitted and is being processed.
    /// * `n < 0` - Which indicates that an overlapped task with a [Code]
    ///             corresponding to `-n` has completed.
    ///
    /// The task of the [lock] function is to find the lock in the open state -
    /// or try and work a lock which is in the negative state atomically into
    /// the open state.
    pub(crate) lock: AtomicI64,
    waker: AtomicWaker,
}

impl Header {
    pub(crate) fn new(max_buffer_size: usize) -> Self {
        Header {
            // Safety: OVERLAPPED structure is valid when zeroed.
            raw: unsafe { mem::MaybeUninit::zeroed().assume_init() },
            pool: BufferPool::new(max_buffer_size),
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
            OverlappedState::Idle
        } else {
            OverlappedState::Pending
        }
    }

    /// Try to lock the overlap and return the a guard to the locked state.
    pub(crate) fn header_lock(&self, Code(id): Code) -> LockResult {
        let id = id as i64 + 1; // to ensure that we never use 0

        let state = loop {
            let op = self
                .lock
                .compare_exchange_weak(0, id, Ordering::SeqCst, Ordering::Relaxed);

            let n = match op {
                Ok(..) => break OverlappedState::Idle,
                Err(n) => n,
            };

            // Already locked, either by our kind of operation or someone else.
            if n > 0 {
                return LockResult::Busy(id != -n);
            }

            // Since we're only racing with a background task and not with
            // others, release the lock into an open state and analyze if the
            // header transitioned from the right code or not.
            self.lock.store(0, Ordering::SeqCst);

            // Here we test if we acquire a lock with the right code or not.
            // If we didn't, we discard the result of the previous operation
            // and treat this as we've just unlocked a new operation.
            //
            // That would be akin to switching in the middle of driving one
            // operation from e.g. a read to a write operation.
            //
            // If we were to just complete the operation, we would use the
            // incorrect contract for it.
            break if id == -n {
                OverlappedState::Pending
            } else {
                OverlappedState::Idle
            };
        };

        LockResult::Ok(state)
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

    /// Unlock access to resources associated with this operation.
    #[inline]
    pub fn unlock_to_open(&self) {
        self.lock.store(0, Ordering::SeqCst);
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

/// The result of trying to lock the overlapped header.
pub(crate) enum LockResult {
    /// Locking succeeded.
    Ok(OverlappedState),
    /// Locking failed because it's busy. Flag indicates if there was a mismatch
    /// in the type of operation.
    ///
    /// If there was a mismatch, the caller should cancel_if_pending the operation.
    Busy(bool),
}
