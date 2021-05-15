use crate::ops::OverlappedOperation;
use crate::overlapped_handle::OverlappedHandle;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::task::{Context, Poll};

/// The internal state of the driver.
#[derive(Debug)]
enum State {
    /// The driver currently owns the operation. The critical section associated
    /// with the operation is owned by the driver.
    Local,
    /// The remote completion port owns the state of the handle.
    Remote,
}

/// Operation helper for turning an [OverlappedOperation] and a [OverlappedHandle]
/// into a pollable operation suitable for use in futures.
#[derive(Debug)]
pub struct Operation<'a, H, O>
where
    H: AsRawHandle,
{
    io: &'a mut OverlappedHandle<H>,
    op: O,
    state: State,
}

impl<'a, H, O> Operation<'a, H, O>
where
    H: AsRawHandle,
{
    /// Construct a new operation wrapper.
    pub fn new(io: &'a mut OverlappedHandle<H>, op: O) -> Self {
        Self {
            io,
            op,
            state: State::Local,
        }
    }

    /// Poll the current operation for completion.
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<O::Output>>
    where
        H: AsRawHandle,
        O: OverlappedOperation<H>,
    {
        let permit = self.io.port.permit()?;
        self.io.register_by_ref(cx.waker());

        let guard = match self.io.header.lock() {
            Some(guard) => guard,
            None => return Poll::Pending,
        };

        match self.state {
            State::Local => {
                let pool = guard.clear_and_get_pool();
                let mut overlapped = guard.overlapped();
                let result = self.op.prepare(&mut self.io.handle, &mut overlapped, pool);
                crate::io::handle_io_pending(result)?;
                std::mem::forget((permit, guard, overlapped));
                self.state = State::Remote;
                Poll::Pending
            }
            State::Remote => {
                self.state = State::Local;
                let result = self.io.result()?;
                Poll::Ready(self.op.collect(result, guard.pool()))
            }
        }
    }
}

impl<H, O> Drop for Operation<'_, H, O>
where
    H: AsRawHandle,
{
    fn drop(&mut self) {
        if let State::Remote = self.state {
            self.io.cancel();
        }
    }
}
