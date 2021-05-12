use crate::ext::HandleExt as _;
use crate::io::IocpHandle;
use crate::io::Overlapped;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::pin::Pin;
use std::task::{Context, Poll};
use winapi::shared::winerror;

/// State that can be used to embed a reader.
#[derive(Debug, Clone, Copy)]
pub enum State {
    Init,
    Pending,
}

impl State {
    fn poll_read<H>(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
        io: &mut IocpHandle<H>,
    ) -> Poll<io::Result<()>>
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

                let mut local_buf = pool.take(buf.remaining());
                let result = io.handle.read_overlapped(&mut local_buf, overlapped);

                match result {
                    Ok(n) => {
                        buf.put_slice(unsafe { local_buf.as_ref(n) });
                        *self = State::Init;
                        Poll::Ready(Ok(()))
                    }
                    Err(e) if e.raw_os_error() == Some(winerror::ERROR_IO_PENDING as i32) => {
                        guard.forget();
                        Poll::Pending
                    }
                    Err(e) => {
                        *self = State::Init;
                        Poll::Ready(Err(e))
                    }
                }
            }
            State::Pending => {
                let result = match io.result() {
                    Ok(result) => {
                        let n = result.bytes_transferred;
                        let local_buf = pool.untake();
                        buf.put_slice(unsafe { local_buf.as_ref(n) });
                        Ok(())
                    }
                    Err(e) => Err(e),
                };

                *self = State::Init;
                Poll::Ready(result)
            }
        }
    }
}

pub struct Reader<'a, H> {
    io: &'a mut IocpHandle<H>,
    state: State,
}

impl<'a, H> Reader<'a, H> {
    pub(crate) fn new(io: &'a mut IocpHandle<H>) -> Self {
        Self {
            io,
            state: State::Init,
        }
    }
}

impl<'a, H> tokio::io::AsyncRead for Reader<'a, H>
where
    H: AsRawHandle,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = unsafe { self.get_unchecked_mut() };
        this.state.poll_read(cx, buf, this.io)
    }
}
