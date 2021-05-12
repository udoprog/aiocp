use parking_lot::Mutex;
use std::mem;
use std::ptr;

/// A buffer that is raw but owned, ready to be submitted through one of the
/// overlapped APIs.
#[derive(Debug, Clone, Copy)]
pub struct RawBuf {
    pub(crate) ptr: *mut u8,
    pub(crate) len: usize,
    cap: usize,
}

impl RawBuf {
    /// Convert back into a vector.
    pub fn into_vec(self) -> Vec<u8> {
        // Safety: the owned buffer can only be correctly allocated inside of
        // this module.
        unsafe { Vec::from_raw_parts(self.ptr, self.len, self.cap) }
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

    /// Zero the current up until the configured length.
    pub fn zero(&mut self) {
        // Safety: zero only up until `len`.
        unsafe { ptr::write_bytes(self.ptr, 0, self.len) }
    }
}

pub struct Pool {
    buffers: Mutex<Vec<Vec<u8>>>,
}

impl Pool {
    /// Construct a new buffer pool.
    pub fn new() -> Self {
        Self {
            buffers: Mutex::new(Vec::new()),
        }
    }

    /// Release the specified buffers.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the buffers referenced are no longer used.
    pub unsafe fn release<const N: usize>(&self, buffers: [RawBuf; N]) {
        let mut locked = self.buffers.lock();

        for buffer in std::array::IntoIter::new(buffers) {
            locked.push(buffer.into_vec());
        }
    }
}

/// Allocate the given buffers.
pub(crate) fn allocate<const N: usize>(caps: [usize; N]) -> [RawBuf; N] {
    let mut buffers = [RawBuf {
        ptr: ptr::null_mut(),
        len: 0,
        cap: 0,
    }; N];

    for (i, cap) in std::array::IntoIter::new(caps).enumerate() {
        let mut buffer = mem::ManuallyDrop::new(Vec::with_capacity(cap));
        let ptr = buffer.as_mut_ptr();

        buffers[i] = RawBuf {
            ptr,
            len: buffer.len(),
            cap: buffer.capacity(),
        };
    }

    buffers
}
