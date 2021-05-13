use crate::atomic_waker::AtomicWaker;
use crate::ioctl;
use crate::ops;
use crate::pool::IocpPool;
use std::cell::UnsafeCell;
use std::convert::TryFrom as _;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::task::Waker;
use winapi::shared::minwindef::FALSE;
use winapi::um::ioapiset;
use winapi::um::minwinbase;

#[cfg(feature = "tokio")]
mod tokio;

mod run;
pub use self::run::Run;

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
pub struct IocpOverlappedHeader {
    raw: UnsafeCell<minwinbase::OVERLAPPED>,
    pool: UnsafeCell<IocpPool>,
    lock: AtomicIsize,
    waker: AtomicWaker,
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

/// Idiomatic wrapper around an overlapped I/O operation.
pub struct IocpHandle<H> {
    pub(crate) handle: H,
    pub(crate) header: Arc<IocpOverlappedHeader>,
}

unsafe impl<H> Send for IocpHandle<H> where H: Send {}
unsafe impl<H> Sync for IocpHandle<H> where H: Sync {}

impl<H> IocpHandle<H> {
    /// Construct a zeroed overlapped structure with the given buffers available
    /// for performing operations over.
    pub(crate) fn new(handle: H) -> Self {
        Self {
            handle,
            header: Arc::new(IocpOverlappedHeader::new()),
        }
    }

    /// Run the given I/O operation while managing it's overlapped
    /// notifications.
    ///
    /// If the operation indicates that it needs to block, the returned future
    /// will complete once the operation has completed.
    pub fn run<O>(&mut self, op: O) -> Run<'_, H, O>
    where
        H: AsRawHandle,
        O: ops::IocpOperation<H>,
    {
        Run::new(self, op)
    }

    /// Access a reference to the underlying handle.
    pub fn as_ref(&self) -> &H {
        &self.handle
    }

    /// Access a mutable reference to the underlying handle.
    pub fn as_mut(&mut self) -> &mut H {
        &mut self.handle
    }

    /// Get the overlapped result from the current structure.
    ///
    /// # Safety
    ///
    /// This must only be called while under an exclusive [lock].
    pub(crate) fn result(&self) -> io::Result<OverlappedResult>
    where
        H: AsRawHandle,
    {
        unsafe {
            let mut bytes_transferred = mem::MaybeUninit::zeroed();

            let result = ioapiset::GetOverlappedResult(
                self.handle.as_raw_handle() as *mut _,
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
    }

    /// Register a new waker.
    pub(crate) fn register_by_ref(&self, waker: &Waker) {
        self.header.waker.register_by_ref(waker);
    }
}

impl<H> IocpHandle<H>
where
    H: AsRawHandle,
{
    /// Write the given buffer and return the number of bytes written.
    pub async fn write<B>(&mut self, buf: B) -> io::Result<usize>
    where
        B: AsRef<[u8]>,
    {
        self.run(ops::Write::new(buf)).await
    }

    /// Write the given buffer and return the number of bytes written.
    pub async fn read<B>(&mut self, buf: B) -> io::Result<usize>
    where
        B: AsMut<[u8]>,
    {
        self.run(ops::Read::new(buf)).await
    }

    /// Connect the current handle, under the assumption that it is the server
    /// side of a named pipe.
    pub async fn connect_named_pipe(&mut self) -> io::Result<()> {
        self.run(ops::ConnectNamedPipe::new()).await
    }

    /// Connect the current handle, under the assumption that it is the server
    /// side of a named pipe.
    pub async fn device_io_control<M>(&mut self, message: M) -> io::Result<usize>
    where
        M: ioctl::Ioctl,
    {
        self.run(ops::DeviceIoCtl::new(message)).await
    }

    /// Coerce into a [Reader][self::tokio::Reader] which implements
    /// [AsyncRead][tokio::io::AsyncRead].
    #[cfg(feature = "tokio")]
    pub fn reader(&mut self) -> self::tokio::Reader<'_, H> {
        self::tokio::Reader::new(self)
    }

    /// Coerce into a [Writer][self::tokio::Writer] which implements
    /// [AsyncWriter][tokio::io::AsyncWriter].
    #[cfg(feature = "tokio")]
    pub fn writer(&mut self) -> self::tokio::Writer<'_, H> {
        self::tokio::Writer::new(self)
    }
}

/// A lock guard that will unlock the resource if dropped.
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
