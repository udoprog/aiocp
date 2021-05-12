use crate::io::{IocpHandle, Overlapped};
use crate::ops;
use std::future::Future;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::pin::Pin;
use std::task::{Context, Poll};
use winapi::shared::winerror;
use winapi::um::ioapiset;

/// The state of the operation being run.
#[derive(Debug, Clone, Copy)]
enum State<O> {
    /// The operation is in its initial state.
    Waiting { op: O },
    /// The operation is pending.
    Pending { op: O },
    /// State of the task is completed.
    Complete,
}

/// An operation performed over the completion port.
pub struct Run<'a, H, O>
where
    H: AsRawHandle,
{
    io: &'a mut IocpHandle<H>,
    state: State<O>,
}

impl<'a, H, O> Run<'a, H, O>
where
    H: AsRawHandle,
{
    /// Construct a new future running the given operation.
    pub(crate) fn new(io: &'a mut IocpHandle<H>, op: O) -> Self {
        Self {
            io,
            state: State::Waiting { op },
        }
    }
}

impl<'a, H, O> Future for Run<'a, H, O>
where
    H: AsRawHandle,
    O: ops::IocpOperation<H>,
{
    type Output = io::Result<O::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        this.io.register_by_ref(cx.waker());

        let (overlapped, mut guard) = match this.io.header.lock() {
            Some(overlapped) => overlapped,
            None => return Poll::Pending,
        };

        let pool = guard.pool();

        match std::mem::replace(&mut this.state, State::Complete) {
            State::Waiting { mut op } => {
                pool.reset();

                let overlapped = Overlapped::from_raw(overlapped);
                let result = op.start(&mut this.io.handle, overlapped, pool);

                match result {
                    Ok(output) => Poll::Ready(Ok(output)),
                    Err(e) if e.raw_os_error() == Some(winerror::ERROR_IO_PENDING as i32) => {
                        guard.forget();
                        this.state = State::Pending { op };
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            State::Pending { mut op } => {
                let result = match this.io.result() {
                    Ok(result) => op.result(result, pool),
                    Err(e) => Err(e),
                };

                pool.reset();
                this.state = State::Complete;
                Poll::Ready(result)
            }
            State::Complete => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::Other,
                "polled after completion",
            ))),
        }
    }
}

impl<'a, H, O> Drop for Run<'a, H, O>
where
    H: AsRawHandle,
{
    fn drop(&mut self) {
        unsafe {
            if let State::Pending { .. } = &mut self.state {
                let _ = ioapiset::CancelIoEx(
                    self.io.handle.as_raw_handle() as *mut _,
                    self.io.header.raw.get(),
                );
            }
        }
    }
}
