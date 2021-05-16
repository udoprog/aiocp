use crate::handle::Handle;
use crate::io::OverlappedState;
use crate::operation::Operation;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::task::{Context, Poll};

/// Operation helper for turning an [Operation] and a [Handle] into something
/// that can be polled inside of a future.
#[derive(Debug)]
pub(crate) struct OperationPoller<'a, H, O>
where
    O: Operation<H>,
    H: AsRawHandle,
{
    io: &'a mut Handle<H>,
    op: O,
}

impl<'a, H, O> OperationPoller<'a, H, O>
where
    O: Operation<H>,
    H: AsRawHandle,
{
    /// Construct a new operation wrapper.
    pub(crate) fn new(io: &'a mut Handle<H>, op: O) -> Self {
        Self { io, op }
    }

    /// Poll the current operation for completion.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<O::Output>>
    where
        H: AsRawHandle,
    {
        let mut guard = match self.io.lock(O::CODE, cx.waker())? {
            Some(guard) => guard,
            None => return Poll::Pending,
        };

        match guard.state() {
            OverlappedState::Idle => {
                let (pool, mut overlapped, handle) = guard.prepare();
                let result = self.op.prepare(handle, &mut overlapped, pool);
                crate::handle::handle_io_pending(result)?;
                std::mem::forget((guard, overlapped));
                Poll::Pending
            }
            OverlappedState::Pending => {
                let result = guard.result()?;
                let (output, outcome) = self.op.collect(result, guard.pool())?;
                outcome.apply_to(&guard);
                Poll::Ready(Ok(output))
            }
        }
    }
}

impl<H, O> Drop for OperationPoller<'_, H, O>
where
    O: Operation<H>,
    H: AsRawHandle,
{
    fn drop(&mut self) {
        if let OverlappedState::Pending = self.io.header.state() {
            self.io.cancel_if_pending();
        }
    }
}
