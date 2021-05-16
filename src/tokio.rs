use crate::ext::HandleExt as _;
use crate::io::OverlappedState;
use crate::ops;
use crate::overlapped_handle::OverlappedHandle;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Wrap a [OverlappedHandle] into a Tokio-compatible type that implements
/// [AsyncRead][tokio::io::AsyncRead] and [AsyncWrite][tokio::io::AsyncWrite].
impl<H> OverlappedHandle<H>
where
    H: AsRawHandle,
{
    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>>
    where
        H: AsRawHandle,
    {
        trace!(op = "read", "poll");
        let permit = self.port.permit()?;
        self.register_by_ref(cx.waker());

        let guard = match self.header.lock(ops::TOKIO_IO) {
            Some(guard) => guard,
            None => return Poll::Pending,
        };

        trace!(op = "read", state = ?guard.state(), "unlocked");

        match guard.state() {
            OverlappedState::Local => {
                let pool = guard.clear_and_get_pool();
                let mut b = pool.take(buf.remaining());
                let mut overlapped = guard.overlapped();
                let result = self.handle.read_overlapped(&mut b, &mut overlapped);
                self.handle_io_pending(result)?;
                std::mem::forget((permit, guard, overlapped));
                Poll::Pending
            }
            OverlappedState::Remote => {
                let pool = guard.pool();
                let result = self.result()?;
                // Safety: this point is synchronized to ensure that no
                // remote buffers are used.
                let b = unsafe { pool.release(result.bytes_transferred) };
                let filled = b.filled();
                buf.put_slice(filled);
                guard.advance(filled.len());
                Poll::Ready(Ok(()))
            }
        }
    }

    fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>
    where
        H: AsRawHandle,
    {
        trace!(op = "write", "poll");
        let permit = self.port.permit()?;
        self.register_by_ref(cx.waker());

        let guard = match self.header.lock(ops::TOKIO_IO) {
            Some(guard) => guard,
            None => return Poll::Pending,
        };

        trace!(op = "write", state = ?guard.state(), "unlocked");

        match guard.state() {
            OverlappedState::Local => {
                let pool = guard.clear_and_get_pool();
                let mut b = pool.take(buf.len());
                b.put_slice(buf);
                let mut overlapped = guard.overlapped();
                let result = self.handle.write_overlapped(b.filled(), &mut overlapped);
                self.handle_io_pending(result)?;
                std::mem::forget((permit, guard, overlapped));
                Poll::Pending
            }
            OverlappedState::Remote => {
                let result = self.result()?;
                guard.advance(result.bytes_transferred);
                Poll::Ready(Ok(result.bytes_transferred))
            }
        }
    }
}

impl<H> AsyncRead for OverlappedHandle<H>
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

    fn cancel_pending_reads(self: Pin<&mut Self>) {
        if let OverlappedState::Remote = self.header.state() {
            self.cancel();
        }
    }
}

impl<H> AsyncWrite for OverlappedHandle<H>
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

    fn cancel_pending_writes(self: Pin<&mut Self>) {
        if let OverlappedState::Remote = self.header.state() {
            self.cancel();
        }
    }
}
