use crate::ext::HandleExt as _;
use crate::io::{Overlapped, OverlappedResult};
use crate::pool::IocpPool;
use std::io;
use std::os::windows::io::AsRawHandle;

/// The model for a single I/O operation.
pub trait IocpOperation<H> {
    type Output;

    /// Start the I/O operation. This typicall runs the function or system call
    /// which requires access to the `OVERLAPPED` structure and memory buffers.
    fn start(
        &mut self,
        handle: &mut H,
        overlapped: Overlapped,
        pool: &mut IocpPool,
    ) -> io::Result<Self::Output>;

    /// Translate the overlapped result into the output of the I/O operation.
    fn result(&mut self, result: OverlappedResult, pool: &mut IocpPool)
        -> io::Result<Self::Output>;
}

/// A write operation wrapping a buffer.
#[derive(Debug)]
pub struct Write<B> {
    buf: B,
}

impl<B> Write<B> {
    /// Construct a new handler for a write operation.
    pub fn new(buf: B) -> Self {
        Self { buf }
    }
}

impl<H, B> IocpOperation<H> for Write<B>
where
    H: AsRawHandle,
    B: AsRef<[u8]>,
{
    type Output = usize;

    fn start(
        &mut self,
        handle: &mut H,
        overlapped: Overlapped,
        pool: &mut IocpPool,
    ) -> io::Result<Self::Output> {
        let buf = self.buf.as_ref();
        let mut b = pool.take(buf.len());
        b.copy_from(buf.as_ref());
        handle.write_overlapped(&mut b, overlapped)
    }

    fn result(&mut self, result: OverlappedResult, _: &mut IocpPool) -> io::Result<Self::Output> {
        // This is executed *after* the overlapped operation has completed.
        Ok(result.bytes_transferred)
    }
}

/// A read operation wrapping a buffer.
#[derive(Debug)]
pub struct Read<B> {
    buf: B,
}

impl<B> Read<B> {
    /// Construct a new handler for a read operation.
    pub fn new(buf: B) -> Self {
        Self { buf }
    }
}

impl<H, B> IocpOperation<H> for Read<B>
where
    H: AsRawHandle,
    B: AsMut<[u8]>,
{
    type Output = usize;

    fn start(
        &mut self,
        handle: &mut H,
        overlapped: Overlapped,
        pool: &mut IocpPool,
    ) -> io::Result<Self::Output> {
        unsafe {
            let mut b = pool.take(self.buf.as_mut().len());
            let n = handle.read_overlapped(&mut b, overlapped)?;
            self.buf.as_mut()[..n].copy_from_slice(b.as_ref(n));
            Ok(n)
        }
    }

    fn result(
        &mut self,
        result: OverlappedResult,
        pool: &mut IocpPool,
    ) -> io::Result<Self::Output> {
        unsafe {
            let n = result.bytes_transferred;
            self.buf.as_mut()[..n].copy_from_slice(pool.untake().as_ref(n));
            Ok(n)
        }
    }
}

/// A write operation wrapping a buffer.
#[derive(Debug)]
pub struct ConnectNamedPipe(());

impl ConnectNamedPipe {
    /// Construct a new handler for a connect operation.
    pub fn new() -> Self {
        Self(())
    }
}

impl<H> IocpOperation<H> for ConnectNamedPipe
where
    H: AsRawHandle,
{
    type Output = ();

    fn start(
        &mut self,
        handle: &mut H,
        overlapped: Overlapped,
        _: &mut IocpPool,
    ) -> io::Result<Self::Output> {
        handle.connect_overlapped(overlapped)
    }

    fn result(&mut self, _: OverlappedResult, _: &mut IocpPool) -> io::Result<Self::Output> {
        Ok(())
    }
}
