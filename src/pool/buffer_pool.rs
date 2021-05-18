use std::cell::{Cell, UnsafeCell};
use std::mem;
use std::slice;
use tokio::io::ReadBuf;

/// A pool of I/O buffers.
pub struct BufferPool {
    buffers: UnsafeCell<Vec<Vec<u8>>>,
    // The number of buffers which are currently busy.
    taken: Cell<usize>,
    // Buffers taken out of the pool.
    released: Cell<usize>,
    // The maximum size permitted for a single I/O buffer.
    max_buffer_size: usize,
}

impl BufferPool {
    /// Construct a new default I/O pool.
    pub(crate) fn new(max_buffer_size: usize) -> Self {
        Self {
            buffers: UnsafeCell::new(vec![vec![0u8; 64]]),
            taken: Cell::new(0),
            released: Cell::new(0),
            max_buffer_size,
        }
    }

    /// Get the max buffer size.
    pub(crate) fn max_buffer_size(&self) -> usize {
        self.max_buffer_size
    }

    /// Take the next I/O buffer that is not busy. Ensures that the returned
    /// buffer can hold at least `size` bytes.
    pub fn take(&self, size: usize) -> ReadBuf<'_> {
        let size = usize::min(size, self.max_buffer_size);

        // Safety: There is no way to access the underlying pool without having
        // an exclusive lock through [OverlappedGuard].
        //
        // We're also checked overlapped access through the local taken /
        // released pair, so that access is never overlapping.
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

            let buf = slice::from_raw_parts_mut(
                buf.as_mut_ptr() as *mut mem::MaybeUninit<u8>,
                buf.capacity(),
            );
            let buf = ReadBuf::uninit(&mut buf[..size]);
            buf
        }
    }

    /// Reclaim the given buffer and decrease increase the number of taken
    /// buffers. Provides a length which indicates the length of the reclaimed
    /// buffer, under the assumption that it has also been filled with the
    /// provided length.
    ///
    /// Buffers are reclaimed in FIFO order. The first call to
    /// [take][BufferPool::take] correponds to the first call of
    /// [release][BufferPool::release].
    pub fn release(&self, len: usize) -> ReadBuf<'_> {
        // Safety: There is no way to access the underlying pool without having
        // an exclusive lock through [OverlappedGuard].
        //
        // We're also checked overlapped access through the local taken /
        // released pair, so that access is never overlapping.
        unsafe {
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
    }

    /// Indicates that there's no pending operations and that this can be safely
    /// reset.
    pub fn clear(&self) {
        self.taken.set(0);
        self.released.set(0);
    }
}
