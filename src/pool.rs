use crate::buf::IocpBuf;

/// A pool of I/O buffers.
pub struct IocpPool {
    buffers: Vec<IocpBuf>,
    // The number of buffers which are currently busy.
    taken: usize,
    // Buffers taken out of the pool.
    released: usize,
}

impl IocpPool {
    /// Construct a new default I/O pool.
    pub(crate) fn new() -> Self {
        Self {
            buffers: vec![IocpBuf::new(64)],
            taken: 0,
            released: 0,
        }
    }

    /// Take the next I/O buffer that is not busy. Ensures that the returned
    /// buffer can hold at least `size` bytes.
    pub fn take(&mut self, size: usize) -> IocpBuf {
        let buf = if self.taken >= self.buffers.len() {
            let buf = IocpBuf::new(size);
            self.buffers.push(unsafe { buf.copy() });
            buf
        } else {
            // Safety: we're ensuring that the buffer is being locally marked as
            // busy just below which prevents further aliasing use.
            unsafe { self.buffers[self.taken].read(size) }
        };

        self.taken += 1;
        buf
    }

    /// Reclaim the given buffer and decrease increase the number of taken
    /// buffers. Provides a length which indicates the length of the reclaimed
    /// buffer.
    ///
    /// # Safety
    ///
    /// Caller must ensure that the taken buffer is initialized to the given
    /// length and that no one else is currently using the buffer.
    pub fn release(&mut self, len: usize) -> IocpBuf {
        assert! {
            self.released < self.taken,
            "released buffers are oob; released = {}, taken = {}",
            self.released,
            self.taken,
        };

        // Safety: we're ensuring that the buffer is being locally marked as
        // busy just below which prevents further aliasing use.
        let mut buf = unsafe { self.buffers[self.released].copy() };
        buf.set_len(len);
        self.released += 1;
        buf
    }

    /// Indicates that there's no pending operations and that this can be safely
    /// reset.
    pub fn reset(&mut self) {
        self.taken = 0;
        self.released = 0;
    }

    pub(crate) unsafe fn drop_in_place(&mut self) {
        for buf in &mut self.buffers {
            buf.drop_in_place();
        }

        self.buffers.set_len(0);
    }
}
