use crate::buf::IocpBuf;

/// A pool of I/O buffers.
pub struct IocpPool {
    buffers: Vec<IocpBuf>,
    // The number of buffers which are currently busy.
    busy: usize,
    // Buffers taken out of the pool.
    taken: usize,
}

impl IocpPool {
    /// Construct a new default I/O pool.
    pub(crate) fn new() -> Self {
        Self {
            buffers: vec![IocpBuf::new(64)],
            busy: 0,
            taken: 0,
        }
    }

    /// Take the next I/O buffer that is not busy. Ensures that the returned
    /// buffer can hold at least `size` bytes.
    pub fn take(&mut self, size: usize) -> IocpBuf {
        let buf = if self.busy >= self.buffers.len() {
            let buf = IocpBuf::new(size);
            self.buffers.push(unsafe { buf.copy() });
            buf
        } else {
            // Safety: we're ensuring that the buffer is being locally marked as
            // busy just below which prevents further aliasing use.
            unsafe { self.buffers[self.busy].read(size) }
        };

        self.busy += 1;
        buf
    }

    /// Reclaim the given buffer and decrease increase the number of taken
    /// buffers.
    pub fn untake(&mut self) -> IocpBuf {
        // Safety: we're ensuring that the buffer is being locally marked as
        // busy just below which prevents further aliasing use.
        let buf = unsafe { self.buffers[self.taken].copy() };
        self.taken += 1;
        buf
    }

    /// Indicates that there's no pending operations and that this can be safely
    /// reset.
    pub fn reset(&mut self) {
        self.busy = 0;
        self.taken = 0;
    }

    pub(crate) unsafe fn drop_in_place(&mut self) {
        for buf in &mut self.buffers {
            buf.drop_in_place();
        }

        self.buffers.set_len(0);
    }
}
