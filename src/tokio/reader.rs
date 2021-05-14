use crate::ext::HandleExt as _;
use crate::io::Overlapped;
use crate::iocp_handle::OverlappedHandle;
use std::io;
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
        io: &mut OverlappedHandle<H>,
    ) -> Poll<io::Result<()>>
    where
        H: AsRawHandle,
    {
        let permit = io.port.permit()?;
        io.register_by_ref(cx.waker());

        let (overlapped, mut guard) = match io.header.lock() {
            Some(overlapped) => overlapped,
            None => return Poll::Pending,
        };

        let pool = guard.pool();

        match *self {
            State::Init => {
                pool.reset();

                let overlapped = Overlapped::from_raw(overlapped);

                let mut b = pool.take(buf.remaining());
                let result = io.handle.read_overlapped(&mut b, overlapped);

                match result {
                    Ok(()) => {
                        buf.put_slice(b.as_ref());
                        Poll::Ready(Ok(()))
                    }
                    Err(e) if e.raw_os_error() == Some(winerror::ERROR_IO_PENDING as i32) => {
                        guard.forget();
                        permit.forget();
                        *self = State::Pending;
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            State::Pending => {
                let result = match io.result() {
                    Ok(result) => {
                        let b = pool.release(result.bytes_transferred);
                        buf.put_slice(b.as_ref());
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
    io: &'a mut OverlappedHandle<H>,
    state: State,
}

impl<'a, H> Reader<'a, H> {
    pub(crate) fn new(io: &'a mut OverlappedHandle<H>) -> Self {
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
