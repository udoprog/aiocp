use crate::ext::HandleExt as _;
use crate::operation::State;
use crate::ops;
use crate::overlapped_handle::OverlappedHandle;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Wrap a [OverlappedHandle] into a Tokio-compatible type that implements
/// [AsyncRead][tokio::io::AsyncRead] and [AsyncWrite][tokio::io::AsyncWrite].
pub fn io<H>(handle: &mut OverlappedHandle<H>) -> Io<'_, H>
where
    H: AsRawHandle,
{
    Io::new(handle)
}

/// A tokio wrapper around the given overlapped handle that enables support for
/// [AsyncRead] and [AsyncWrite].
pub struct Io<'a, H>
where
    H: AsRawHandle,
{
    state: State,
    io: &'a mut OverlappedHandle<H>,
}

impl<'a, H> Io<'a, H>
where
    H: AsRawHandle,
{
    fn new(io: &'a mut OverlappedHandle<H>) -> Self {
        Self {
            state: State::Local,
            io,
        }
    }

    /// Get a reference to the underlying overlapped handle.
    pub fn as_ref(&self) -> &OverlappedHandle<H> {
        self.io
    }

    /// Get a mutable reference to the underlying overlapped handle.
    pub fn as_mut(&mut self) -> &mut OverlappedHandle<H> {
        self.io
    }

    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>>
    where
        H: AsRawHandle,
    {
        trace!(op = "read", "poll");
        let permit = self.io.port.permit()?;
        self.io.register_by_ref(cx.waker());

        let guard = match self.io.header.lock(ops::READ) {
            Some(guard) => guard,
            None => return Poll::Pending,
        };

        match self.state {
            State::Local => {
                let pool = guard.clear_and_get_pool();
                let mut b = pool.take(buf.remaining());
                let mut overlapped = guard.overlapped();
                let result = self.io.handle.read_overlapped(&mut b, &mut overlapped);
                crate::io::handle_io_pending(result)?;
                std::mem::forget((permit, guard, overlapped));
                self.state = State::Remote;
                Poll::Pending
            }
            State::Remote => {
                self.state = State::Local;
                let pool = guard.pool();
                let result = self.io.result()?;
                // Safety: this point is synchronized to ensure that no
                // remote buffers are used.
                let b = unsafe { pool.release(result.bytes_transferred) };
                buf.put_slice(b.filled());
                Poll::Ready(Ok(()))
            }
        }
    }

    fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>
    where
        H: AsRawHandle,
    {
        trace!(op = "write", "poll");
        let permit = self.io.port.permit()?;
        self.io.register_by_ref(cx.waker());

        let guard = match self.io.header.lock(ops::WRITE) {
            Some(guard) => guard,
            None => return Poll::Pending,
        };

        match self.state {
            State::Local => {
                let pool = guard.clear_and_get_pool();
                let mut b = pool.take(buf.len());
                b.put_slice(buf);
                let mut overlapped = guard.overlapped();
                let result = self.io.handle.write_overlapped(&b, &mut overlapped);
                crate::io::handle_io_pending(result)?;
                std::mem::forget((permit, guard, overlapped));
                self.state = State::Remote;
                Poll::Pending
            }
            State::Remote => {
                self.state = State::Local;
                let result = self.io.result()?;
                Poll::Ready(Ok(result.bytes_transferred))
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
