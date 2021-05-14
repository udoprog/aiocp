use std::mem;
use std::ptr;
use std::slice;

/// A buffer that is raw but owned, ready to be submitted through one of the
/// overlapped APIs.
#[derive(Debug)]
pub struct Buf {
    ptr: *mut u8,
    len: usize,
    cap: usize,
}

impl Buf {
    /// Construct a new empty buffer suitable for submitting to I/O Completion
    /// Ports.
    pub(crate) fn new(cap: usize) -> Self {
        let mut buffer = mem::ManuallyDrop::new(vec![0; cap]);
        let ptr = buffer.as_mut_ptr();

        Self {
            ptr,
            len: buffer.len(),
            cap: buffer.capacity(),
        }
    }

    /// The current length of the buffer.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Set the written to length. Note that setting the length beyond something
    /// that has been written results in cluttered bytes being made visible.
    ///
    /// But they will never be uninitialized, so it's not unsafe.
    pub(crate) fn set_len(&mut self, len: usize) {
        assert!(
            len <= self.cap,
            "updated len is oob; len = {}, cap = {}",
            len,
            self.cap
        );

        self.len = len;
    }

    /// Access a pointer to the underlying buffer.
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr as *const _
    }

    /// Access a mutable pointer to the underlying buffer.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    /// Access the underlying buffer mutably with the given len.
    pub fn as_ref(&self) -> &[u8] {
        // Safety: all avenues of modifying the underlying buffer are checked or
        // unsafe.
        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }

    /// Copy from the specified bytes into the current buffer.
    pub fn copy_from(&mut self, from: &[u8]) {
        assert! {
            from.len() <= self.cap,
            "buffer capacity is smaller than required length; cap = {}, len = {}",
            self.cap, from.len(),
        };

        unsafe {
            ptr::copy_nonoverlapping(from.as_ptr(), self.ptr, from.len());
        }

        self.len = from.len();
    }

    /// Zero the current buffer up until its given length and zero its length.
    pub fn zero(&mut self) {
        // Safety: zero only up until `len`.
        unsafe { ptr::write_bytes(self.ptr, 0, self.len) }
        self.len = 0;
    }

    /// Get a copy of the current buffer.
    ///
    /// # Safety
    ///
    /// Caller must ensure that the buffer being copied is being "owner
    /// transferred" somewhere, so that the buffer is never aliased.
    pub(crate) unsafe fn copy(&self) -> Self {
        ptr::read(self)
    }

    /// Read and modify the buffer in place, before returning an owned copy of
    /// it.
    ///
    /// # Safety
    ///
    /// Caller must ensure that the buffer being copied is being "owner
    /// transferred" somewhere, so that the buffer is never aliased.
    pub(crate) unsafe fn read(&mut self, len: usize) -> Self {
        if self.cap < len {
            let mut vec = mem::ManuallyDrop::new(Vec::from_raw_parts(self.ptr, self.len, self.cap));
            vec.reserve(len - self.cap);

            self.ptr = vec.as_mut_ptr();
            self.cap = vec.capacity();
        }

        self.len = len;
        ptr::read(self)
    }

    /// Drop the current buffer in place.
    pub(crate) unsafe fn drop_in_place(&mut self) {
        let _ = Vec::from_raw_parts(self.ptr, self.len, self.cap);
    }
}
