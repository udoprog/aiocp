use crate::operation;
use crate::pool::BufferPool;
use crate::task::Header;
use crate::task::LockResult;
use std::convert::TryFrom as _;
use std::io;
use std::marker;
use std::mem;
use std::sync::Arc;
use std::task::Waker;
use winapi::ctypes::c_void;
use winapi::shared::minwindef::FALSE;
use winapi::shared::winerror;
use winapi::um::errhandlingapi;
use winapi::um::ioapiset;
use winapi::um::minwinbase;

pub(crate) trait Flavor<T> {
    fn into_handle(handle: &T) -> *mut c_void;
}

pub(crate) struct Internal<T, F>
where
    F: Flavor<T>,
{
    pub(crate) handle: T,
    pub(crate) port: crate::sys::CompletionPort,
    pub(crate) header: Arc<Header>,
    pub(crate) _marker: marker::PhantomData<F>,
}

impl<T, F> Internal<T, F>
where
    F: Flavor<T>,
{
    pub(crate) fn new(handle: T, port: crate::sys::CompletionPort, max_buffer_size: usize) -> Self {
        Self {
            handle,
            port,
            header: Arc::new(Header::new(max_buffer_size)),
            _marker: marker::PhantomData,
        }
    }

    pub(crate) fn duplicate(&self) -> Self
    where
        T: Clone,
    {
        let max_buffer_size = self.header.pool.max_buffer_size();

        Self {
            handle: self.handle.clone(),
            port: self.port.clone(),
            header: Arc::new(Header::new(max_buffer_size)),
            _marker: marker::PhantomData,
        }
    }

    /// Access a reference to the underlying handle.
    pub(crate) fn as_ref(&self) -> &T {
        &self.handle
    }

    /// Access a mutable reference to the underlying handle.
    pub(crate) fn as_mut(&mut self) -> &mut T {
        &mut self.handle
    }

    /// Register a new waker.
    pub(crate) fn register_by_ref(&self, waker: &Waker) {
        self.header.register_by_ref(waker);
    }

    /// Lock the current handle.
    pub(crate) fn lock<'a>(
        &'a mut self,
        code: operation::Code,
        waker: &Waker,
    ) -> io::Result<Option<LockGuard<'a, T, F>>> {
        let permit = self.port.permit()?;
        self.register_by_ref(waker);

        Ok(match self.header.header_lock(code) {
            LockResult::Ok(state) => Some(LockGuard {
                handle: &mut self.handle,
                header: &self.header,
                state,
                _marker: marker::PhantomData,
                _permit: permit,
            }),
            LockResult::Busy(mismatch) => {
                if mismatch {
                    self.cancel_immediate();
                }

                None
            }
        })
    }

    /// Cancel if we know there's an operation running in the background.
    pub(crate) fn cancel_if_pending(&self) {
        if let OverlappedState::Pending = self.header.state() {
            self.cancel_immediate();
        }
    }

    /// Asynchronously cancel_if_pending an pending I/O operation associated with this
    /// handle on the I/O completion port.
    ///
    /// # Safety
    ///
    /// Issue a cancellation.
    fn cancel_immediate(&self) {
        // Safety: there's nothing inherently unsafe about this, both the header
        // pointer and the local handle are guaranteed to be alive for the
        // duration of this object.
        unsafe {
            let ptr = F::into_handle(&self.handle);
            let _ = ioapiset::CancelIoEx(ptr, self.header.as_raw_overlapped());
        }
    }
}

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

/// A lock guard that will unlock the resource if dropped.
///
/// Dropping the guard automatically clears the available buffers and unlocks
/// the header.
pub(crate) struct LockGuard<'a, H, F>
where
    F: Flavor<H>,
{
    handle: &'a mut H,
    header: &'a Arc<Header>,
    state: OverlappedState,
    _marker: marker::PhantomData<F>,
    _permit: crate::sys::CompletionPortPermit<'a>,
}

impl<'a, H, F> LockGuard<'a, H, F>
where
    F: Flavor<H>,
{
    /// Get the overlapped result from the current structure.
    pub(crate) fn handle_result(&self) -> io::Result<OverlappedResult> {
        // Safety: All of there operations are safe and it is guaranteed that
        // we have access to the underlying handles.
        unsafe {
            let mut bytes_transferred = mem::MaybeUninit::zeroed();

            let result = ioapiset::GetOverlappedResult(
                F::into_handle(self.handle),
                self.header.as_raw_overlapped(),
                bytes_transferred.as_mut_ptr(),
                FALSE,
            );

            if result == FALSE {
                return match errhandlingapi::GetLastError() {
                    winerror::ERROR_HANDLE_EOF => Ok(OverlappedResult::empty()),
                    winerror::ERROR_BROKEN_PIPE => Ok(OverlappedResult::empty()),
                    other => Err(io::Error::from_raw_os_error(other as i32)),
                };
            }

            let bytes_transferred = usize::try_from(bytes_transferred.assume_init())
                .expect("bytes transferred out of bounds");

            Ok(OverlappedResult { bytes_transferred })
        }
    }
}

impl<H, F> LockGuard<'_, H, F>
where
    F: Flavor<H>,
{
    /// Prepare the information.
    pub fn prepare(&mut self) -> (&BufferPool, Overlapped, &mut H) {
        let pool = &self.header.pool;
        pool.clear();

        let overlapped = Overlapped::from_raw(Arc::into_raw(self.header.clone())
            as *const minwinbase::OVERLAPPED
            as *mut _);

        (pool, overlapped, &mut *self.handle)
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
}

impl<H, I> Drop for LockGuard<'_, H, I>
where
    I: Flavor<H>,
{
    fn drop(&mut self) {
        self.header.pool.clear();
        self.header.unlock_to_open();
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
