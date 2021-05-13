use crate::io::Overlapped;
use crate::iocp_handle::IocpHandle;
use crate::ops::IocpOperation;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::task::{Context, Poll};
use winapi::shared::winerror;
use winapi::um::ioapiset;

/// The internal state of the driver.
#[derive(Debug)]
enum State {
    /// The driver currently owns the operation. The critical section associated
    /// with the operation is owned by the driver.
    Local,
    /// The remote completion port owns the state of the handle.
    Remote,
}

/// Operation helper for turning an [IocpOperation] and a [IocpHandle] into a
/// pollable operation suitable for use in futures.
#[derive(Debug)]
pub struct Operation<'a, H, O>
where
    H: AsRawHandle,
{
    io: &'a mut IocpHandle<H>,
    op: O,
    state: State,
}

impl<'a, H, O> Operation<'a, H, O>
where
    O: IocpOperation<H>,
    H: AsRawHandle,
{
    /// Construct a new operation wrapper.
    pub fn new(io: &'a mut IocpHandle<H>, op: O) -> Self {
        Self {
            io,
            op,
            state: State::Local,
        }
    }

    /// Poll the driver.
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<O::Output>>
    where
        H: AsRawHandle,
    {
        self.io.register_by_ref(cx.waker());

        let (overlapped, mut guard) = match self.io.header.lock() {
            Some(overlapped) => overlapped,
            None => return Poll::Pending,
        };

        let pool = guard.pool();

        match self.state {
            State::Local => {
                pool.reset();

                let overlapped = Overlapped::from_raw(overlapped);
                let result = self.op.start(&mut self.io.handle, overlapped, pool);

                match result {
                    Ok(output) => Poll::Ready(Ok(output)),
                    Err(e) if e.raw_os_error() == Some(winerror::ERROR_IO_PENDING as i32) => {
                        guard.forget();
                        self.state = State::Remote;
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            State::Remote => {
                let result = match self.io.result() {
                    Ok(result) => self.op.result(result, pool),
                    Err(e) => Err(e),
                };

                pool.reset();
                self.state = State::Local;
                Poll::Ready(result)
            }
        }
    }
}

impl<H, O> Drop for Operation<'_, H, O>
where
    H: AsRawHandle,
{
    fn drop(&mut self) {
        unsafe {
            if let State::Remote = self.state {
                let _ = ioapiset::CancelIoEx(
                    self.io.handle.as_raw_handle() as *mut _,
                    self.io.header.raw.get(),
                );
            }
        }
    }
}
