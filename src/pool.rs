use std::cell::Cell;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::slice;
use tokio::io::ReadBuf;

/// A pool of I/O buffers.
pub struct BufferPool {
    buffers: UnsafeCell<Vec<Vec<u8>>>,
    // The number of buffers which are currently busy.
    taken: Cell<usize>,
    // Buffers taken out of the pool.
    released: Cell<usize>,
}

impl BufferPool {
    /// Construct a new default I/O pool.
    pub(crate) fn new() -> Self {
        Self {
            buffers: UnsafeCell::new(vec![vec![0u8; 64]]),
            taken: Cell::new(0),
            released: Cell::new(0),
        }
    }

    /// Take the next I/O buffer that is not busy. Ensures that the returned
    /// buffer can hold at least `size` bytes.
    pub fn take(&self, size: usize) -> ReadBuf<'_> {
        // Safety: we're ensuring that the buffer is being locally marked as
        // busy through `taken` just below which prevents further aliasing use.
        unsafe {
            let buffers = &mut *self.buffers.get();

            let taken = self.taken.get();
            self.taken.set(taken + 1);

            if taken >= buffers.len() {
                buffers.push(Vec::with_capacity(size));
            }

            let buf = &mut buffers[taken];

            if buf.capacity() < size {
                buf.reserve(size - buf.capacity());
            }

            let buf =
                slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut MaybeUninit<u8>, buf.capacity());
            let buf = ReadBuf::uninit(buf);
            buf
        }
    }

    /// Reclaim the given buffer and decrease increase the number of taken
    /// buffers. Provides a length which indicates the length of the reclaimed
    /// buffer, under the assumption that it has also been filled with the
    /// provided length.
    ///
    /// # Safety
    ///
    /// Caller must ensure that the taken buffer is initialized to the given
    /// length and that the buffer is not currently in used.
    ///
    /// The caller must also ensure that the released buffer has been filled up
    /// to `len`.
    pub unsafe fn release(&self, len: usize) -> ReadBuf<'_> {
        let buffers = &mut *self.buffers.get();

        let released = self.released.get();
        self.released.set(released + 1);

        assert! {
            released < self.taken.get(),
            "released buffers are oob; released = {}, taken = {}",
            released,
            self.taken.get(),
        };

        let buf = &mut buffers[released];

        assert! {
            len <= buf.capacity(),
            "buffer length out of bounds for capacity of buffer; len = {}, cap = {}",
            len,
            buf.capacity(),
        };

        let buf = slice::from_raw_parts_mut(buf.as_mut_ptr(), len);
        let mut buf = ReadBuf::new(buf);
        buf.set_filled(len);
        buf
    }

    /// Indicates that there's no pending operations and that this can be safely
    /// reset.
    pub fn clear(&self) {
        self.taken.set(0);
        self.released.set(0);
    }
}
