use crate::ext::HandleExt as _;
use crate::io::Overlapped;
use crate::ioctl;
use crate::overlapped_handle::OverlappedResult;
use crate::pool::BufferPool;
use std::io;
use std::os::windows::io::AsRawHandle;

/// The model for a single I/O operation.
pub trait OverlappedOperation<H> {
    /// The output of the operation.
    type Output;

    /// Prepare the I/O operation. This typicall runs the function or system
    /// call which requires access to the `OVERLAPPED` structure and memory
    /// buffers.
    fn prepare(
        &mut self,
        handle: &mut H,
        overlapped: &mut Overlapped,
        pool: &BufferPool,
    ) -> io::Result<()>;

    /// Collect the overlapped result into the output of the I/O operation.
    fn collect(&mut self, result: OverlappedResult, pool: &BufferPool) -> io::Result<Self::Output>;
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

impl<H, B> OverlappedOperation<H> for Write<B>
where
    H: AsRawHandle,
    B: AsRef<[u8]>,
{
    type Output = usize;

    fn prepare(
        &mut self,
        handle: &mut H,
        overlapped: &mut Overlapped,
        pool: &BufferPool,
    ) -> io::Result<()> {
        let buf = self.buf.as_ref();
        let mut b = pool.take(buf.len());
        b.put_slice(buf);
        handle.write_overlapped(&mut b, overlapped)?;
        Ok(())
    }

    fn collect(&mut self, result: OverlappedResult, _: &BufferPool) -> io::Result<Self::Output> {
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

impl<H, B> OverlappedOperation<H> for Read<B>
where
    H: AsRawHandle,
    B: AsMut<[u8]>,
{
    type Output = usize;

    fn prepare(
        &mut self,
        handle: &mut H,
        overlapped: &mut Overlapped,
        pool: &BufferPool,
    ) -> io::Result<()> {
        let mut b = pool.take(self.buf.as_mut().len());
        handle.read_overlapped(&mut b, overlapped)?;
        Ok(())
    }

    fn collect(&mut self, result: OverlappedResult, pool: &BufferPool) -> io::Result<Self::Output> {
        let b = unsafe { pool.release(result.bytes_transferred) };
        let filled = b.filled();
        self.buf.as_mut()[..filled.len()].copy_from_slice(filled);
        Ok(filled.len())
    }
}

/// A write named pipe connect.
#[derive(Debug)]
pub struct ConnectNamedPipe(());

impl ConnectNamedPipe {
    /// Construct a new handler for a connect operation.
    pub fn new() -> Self {
        Self(())
    }
}

impl<H> OverlappedOperation<H> for ConnectNamedPipe
where
    H: AsRawHandle,
{
    type Output = ();

    fn prepare(
        &mut self,
        handle: &mut H,
        overlapped: &mut Overlapped,
        _: &BufferPool,
    ) -> io::Result<Self::Output> {
        handle.connect_overlapped(overlapped)
    }

    fn collect(&mut self, _: OverlappedResult, _: &BufferPool) -> io::Result<Self::Output> {
        Ok(())
    }
}

/// A write operation wrapping a buffer.
#[derive(Debug)]
pub struct DeviceIoCtl<M>(M);

impl<M> DeviceIoCtl<M> {
    /// Construct a new handler for a connect operation.
    pub fn new(message: M) -> Self {
        Self(message)
    }
}

impl<H, M> OverlappedOperation<H> for DeviceIoCtl<M>
where
    H: AsRawHandle,
    M: ioctl::Ioctl,
{
    type Output = usize;

    fn prepare(
        &mut self,
        handle: &mut H,
        overlapped: &mut Overlapped,
        pool: &BufferPool,
    ) -> io::Result<()> {
        use std::slice;

        let len = self.0.len();
        let mut buf = pool.take(len);
        // serialize request.
        buf.put_slice(unsafe { slice::from_raw_parts(&self.0 as *const _ as *const u8, len) });
        handle.device_io_control_overlapped(M::CONTROL, Some(&mut buf), None, overlapped)?;
        Ok(())
    }

    fn collect(&mut self, result: OverlappedResult, _: &BufferPool) -> io::Result<Self::Output> {
        Ok(result.bytes_transferred)
    }
}
