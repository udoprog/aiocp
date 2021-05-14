use crate::ext::HandleExt as _;
use crate::io::Overlapped;
use crate::iocp_handle::OverlappedHandle;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use winapi::shared::winerror;

/// State that can be used to embed a reader.
#[derive(Debug, Clone, Copy)]
enum State {
    Local,
    Remote,
}

/// A tokio wrapper around the given overlapped handle that enables support for
/// [AsyncRead] and [AsyncWrite].
pub struct Io<'a, H>
where
    H: AsRawHandle,
{
    io: &'a mut OverlappedHandle<H>,
    state: State,
}

impl<'a, H> Io<'a, H>
where
    H: AsRawHandle,
{
    /// Wrap a [OverlappedHandle] into a Tokio-compatible type that implements
    /// [AsyncRead][tokio::io::AsyncRead] and
    /// [AsyncWrite][tokio::io::AsyncWrite].
    pub fn new(io: &'a mut OverlappedHandle<H>) -> Self {
        Self {
            io,
            state: State::Local,
        }
    }

    fn poll_read(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>>
    where
        H: AsRawHandle,
    {
        let permit = self.io.port.permit()?;
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

                let mut b = pool.take(buf.remaining());
                let result = self.io.handle.read_overlapped(&mut b, overlapped);

                match result {
                    Ok(()) => {
                        buf.put_slice(b.as_ref());
                        Poll::Ready(Ok(()))
                    }
                    Err(e) if e.raw_os_error() == Some(winerror::ERROR_IO_PENDING as i32) => {
                        guard.forget();
                        permit.forget();
                        self.state = State::Remote;
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            State::Remote => {
                let result = match self.io.result() {
                    Ok(result) => {
                        let b = pool.release(result.bytes_transferred);
                        buf.put_slice(b.as_ref());
                        Ok(())
                    }
                    Err(e) => Err(e),
                };

                self.state = State::Local;
                Poll::Ready(result)
            }
        }
    }

    fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>
    where
        H: AsRawHandle,
    {
        let permit = self.io.port.permit()?;
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

                let mut b = pool.take(buf.len());
                b.copy_from(buf);
                let result = self.io.handle.write_overlapped(&b, overlapped);

                match result {
                    Ok(n) => Poll::Ready(Ok(n)),
                    Err(e) if e.raw_os_error() == Some(winerror::ERROR_IO_PENDING as i32) => {
                        guard.forget();
                        permit.forget();
                        self.state = State::Remote;
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            State::Remote => {
                // Safety: we're holding the exclusive lock.
                let result = self.io.result();

                let result = match result {
                    Ok(result) => Ok(result.bytes_transferred),
                    Err(e) => Err(e),
                };

                self.state = State::Local;
                Poll::Ready(result)
            }
        }
    }
}

impl<H> AsyncRead for Io<'_, H>
where
    H: AsRawHandle,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = unsafe { self.get_unchecked_mut() };
        this.poll_read(cx, buf)
    }
}

impl<H> AsyncWrite for Io<'_, H>
where
    H: AsRawHandle,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let this = unsafe { self.get_unchecked_mut() };
        this.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl<H> Drop for Io<'_, H>
where
    H: AsRawHandle,
{
    fn drop(&mut self) {
        if let State::Remote = self.state {
            self.io.cancel();
        }
    }
}
