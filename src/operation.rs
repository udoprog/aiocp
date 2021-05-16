use crate::handle::Handle;
use crate::io::OverlappedState;
use crate::ops::OverlappedOperation;
use crate::task::LockResult;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::task::{Context, Poll};

/// Operation helper for turning an [OverlappedOperation] and a [Handle]
/// into a pollable operation suitable for use in futures.
#[derive(Debug)]
pub(crate) struct Operation<'a, H, O>
where
    O: OverlappedOperation<H>,
    H: AsRawHandle,
{
    io: &'a mut Handle<H>,
    op: O,
}

impl<'a, H, O> Operation<'a, H, O>
where
    O: OverlappedOperation<H>,
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
        let permit = self.io.port.permit()?;
        self.io.register_by_ref(cx.waker());

        let guard = match self.io.header.lock(O::CODE) {
            LockResult::Ok(guard) => guard,
            LockResult::Busy(mismatch) => {
                if mismatch {
                    self.io.cancel_immediate();
                }

                return Poll::Pending;
            }
        };

        match guard.state() {
            OverlappedState::Local => {
                let pool = guard.clear_and_get_pool();
                let mut overlapped = guard.overlapped();
                let result = self.op.prepare(&mut self.io.handle, &mut overlapped, pool);
                self.io.handle_io_pending(result)?;
                std::mem::forget((permit, guard, overlapped));
                Poll::Pending
            }
            OverlappedState::Remote => {
                let result = self.io.result()?;
                let (output, outcome) = self.op.collect(result, guard.pool())?;
                outcome.apply_to(&guard);
                Poll::Ready(Ok(output))
            }
        }
    }
}

impl<H, O> Drop for Operation<'_, H, O>
where
    O: OverlappedOperation<H>,
    H: AsRawHandle,
{
    fn drop(&mut self) {
        if let OverlappedState::Remote = self.io.header.state() {
            self.io.cancel_if_pending();
        }
    }
}
