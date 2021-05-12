use crate::ext::HandleExt as _;
use crate::io::{IocpHandle, Overlapped};
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::pin::Pin;
use std::task::{Context, Poll};
use winapi::shared::winerror;

/// State that can be used to embed a writer.
#[derive(Debug, Clone, Copy)]
pub enum State {
    Init,
    Pending,
}

impl State {
    pub fn poll_write<H>(
        &mut self,
        cx: &mut Context<'_>,
        buf: &[u8],
        io: &mut IocpHandle<H>,
    ) -> Poll<io::Result<usize>>
    where
        H: AsRawHandle,
    {
        io.register_by_ref(cx.waker());

        let (overlapped, mut guard) = match io.header.lock() {
            Some(overlapped) => overlapped,
            None => return Poll::Pending,
        };

        let pool = guard.pool();

        match mem::replace(self, State::Pending) {
            State::Init => {
                pool.reset();

                let overlapped = Overlapped::from_raw(overlapped);

                let mut local_buf = pool.take(buf.len());
                local_buf.copy_from(buf);
                let result = io.handle.write_overlapped(&local_buf, overlapped);

                match result {
                    Ok(n) => {
                        *self = State::Init;
                        Poll::Ready(Ok(n))
                    }
                    Err(e) if e.raw_os_error() == Some(winerror::ERROR_IO_PENDING as i32) => {
                        guard.forget();
                        *self = State::Pending;
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            State::Pending => {
                // Safety: we're holding the exclusive lock.
                let result = io.result();

                let result = match result {
                    Ok(result) => Ok(result.bytes_transferred),
                    Err(e) => Err(e),
                };

                *self = State::Init;
                Poll::Ready(result)
            }
        }
    }
}

pub struct Writer<'a, H> {
    io: &'a mut IocpHandle<H>,
    state: State,
}

impl<'a, H> Writer<'a, H> {
    pub(crate) fn new(io: &'a mut IocpHandle<H>) -> Self {
        Self {
            io,
            state: State::Init,
        }
    }
}

impl<'a, H> tokio::io::AsyncWrite for Writer<'a, H>
where
    H: AsRawHandle,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let this = unsafe { self.get_unchecked_mut() };
        this.state.poll_write(cx, buf, this.io)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }
}
