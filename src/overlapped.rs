use crate::atomic_waker::AtomicWaker;
use crate::buf::RawBuf;
use parking_lot::lock_api::RawMutex as _;
use pin_project::pin_project;
use std::cell::UnsafeCell;
use std::convert::TryFrom as _;
use std::future::Future;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;
use std::task::Waker;
use std::task::{Context, Poll};
use winapi::shared::minwindef::FALSE;
use winapi::shared::winerror;
use winapi::um::ioapiset;
use winapi::um::minwinbase;

/// A raw overlapped structure.
#[derive(Debug, Clone, Copy)]
pub struct RawOverlapped(pub(crate) *mut minwinbase::OVERLAPPED);

#[repr(C)]
pub(crate) struct OverlappedHeader {
    pub(crate) raw: UnsafeCell<minwinbase::OVERLAPPED>,
    pub(crate) lock: parking_lot::RawMutex,
    pub(crate) waker: AtomicWaker,
}

impl OverlappedHeader {
    pub(crate) fn new() -> Self {
        OverlappedHeader {
            // Safety: OVERLAPPED structure is valid when zeroed.
            raw: unsafe { mem::MaybeUninit::zeroed().assume_init() },
            lock: parking_lot::RawMutex::INIT,
            waker: AtomicWaker::new(),
        }
    }

    /// Construct from the given overlapped pointer.
    pub(crate) unsafe fn from_overlapped(overlapped: *const minwinbase::OVERLAPPED) -> Arc<Self> {
        Arc::from_raw(overlapped as *const Self)
    }

    /// Try to unlock the overlapped struct.
    pub(crate) unsafe fn unlock(&self) {
        self.lock.unlock()
    }

    /// Wake the currently registered waker.
    pub(crate) fn wake(&self) {
        self.waker.wake();
    }
}

/// Idiomatic wrapper around an overlapped operation.
pub struct Overlapped<const N: usize> {
    pub(crate) header: Arc<OverlappedHeader>,
    pub(crate) buffers: [RawBuf; N],
}

impl<const N: usize> Overlapped<N> {
    /// Construct a zeroed overlapped structure with the given buffers available
    /// for performing operations over.
    pub fn new(caps: [usize; N]) -> Self {
        Self {
            header: Arc::new(OverlappedHeader::new()),
            buffers: crate::buf::allocate(caps),
        }
    }

    /// Perform the given operation under an overlapped operation
    /// asynchronously.
    ///
    /// If the operation indicates that it needs to block, the returned future
    /// will complete once the operation has completed.
    pub unsafe fn perform<'a, H, T, R, O>(
        &'a self,
        handle: &'a mut H,
        op: T,
        result: R,
    ) -> Perform<'a, H, T, R, N>
    where
        T: FnOnce(&mut H, RawOverlapped, [RawBuf; N]) -> io::Result<O>,
        R: FnMut(OverlappedResult) -> io::Result<O>,
    {
        Perform {
            overlapped: self,
            handle,
            op: Some(op),
            result,
        }
    }

    /// Get the overlapped result from the current structure.
    ///
    /// # Safety
    ///
    /// This must only be called while under an exclusive [lock].
    pub(crate) unsafe fn result<H>(&self, handle: &H) -> io::Result<OverlappedResult>
    where
        H: AsRawHandle,
    {
        let mut bytes_transferred = mem::MaybeUninit::zeroed();

        let result = ioapiset::GetOverlappedResult(
            handle.as_raw_handle(),
            self.header.raw.get(),
            bytes_transferred.as_mut_ptr(),
            FALSE,
        );

        if result == FALSE {
            return Err(io::Error::last_os_error());
        }

        let bytes_transferred = usize::try_from(bytes_transferred.assume_init())
            .expect("bytes transferred out of bounds");

        Ok(OverlappedResult { bytes_transferred })
    }

    /// Try to lock the overlap and return the pointer to the associated
    /// overlapped struct.
    pub(crate) fn lock(&self) -> Option<(*mut minwinbase::OVERLAPPED, [RawBuf; N])> {
        if self.header.lock.try_lock() {
            let overlapped =
                Arc::into_raw(self.header.clone()) as *const minwinbase::OVERLAPPED as *mut _;

            return Some((overlapped, self.buffers));
        }

        None
    }

    /// Register a new waker.
    pub(crate) fn register_by_ref(&self, waker: &Waker) {
        self.header.waker.register_by_ref(waker);
    }
}

/// An operation performed over the completion port.
#[pin_project]
pub struct Perform<'a, H, T, R, const N: usize> {
    overlapped: &'a Overlapped<N>,
    handle: &'a mut H,
    op: Option<T>,
    result: R,
}

impl<'a, H, T, R, O, const N: usize> Future for Perform<'a, H, T, R, N>
where
    H: AsRawHandle,
    T: FnOnce(&mut H, RawOverlapped, [RawBuf; N]) -> io::Result<O>,
    R: FnMut(OverlappedResult) -> io::Result<O>,
{
    type Output = io::Result<O>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        // We've just inserted the overlapped value so we're guaranteed to be
        // referencing an internal overlapped handler.
        this.overlapped.register_by_ref(cx.waker());

        let (overlapped, mut buffers) = match this.overlapped.lock() {
            Some(locked) => locked,
            None => return Poll::Pending,
        };

        let op = match this.op.take() {
            Some(op) => op,
            None => {
                // Safety: we're holding the exclusive lock.
                let result = unsafe { this.overlapped.result(*this.handle)? };
                return Poll::Ready((this.result)(result));
            }
        };

        for buffer in &mut buffers {
            buffer.zero();
        }

        let result = op(this.handle, RawOverlapped(overlapped), buffers);

        let e = match result {
            Ok(output) => return Poll::Ready(Ok(output)),
            Err(e) => e,
        };

        let os = match e.raw_os_error() {
            Some(os) => os as u32,
            None => return Poll::Ready(Err(e)),
        };

        match os as u32 {
            winerror::ERROR_IO_PENDING => Poll::Pending,
            _ => Poll::Ready(Err(e)),
        }
    }
}

/// The results of an overlapped operation.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct OverlappedResult {
    pub bytes_transferred: usize,
}
