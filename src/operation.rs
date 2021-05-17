//! Abstractions for building raw overlapping operation helpers.

use crate::ext::HandleExt as _;
use crate::io::{Flavor, LockGuard, Overlapped, OverlappedResult};
use crate::ioctl;
use crate::pool::BufferPool;
use crate::sys::AsRawHandle;
use std::io;

/// The lock code for read-like operations.
///
/// If you're using this, you must ensure that:
///
/// * Exactly one buffer is used from the pool and that it is *appropriately
///   sized* for what is being read. Every byte must fit, and
///   [OverlappedResult::bytes_transferred] must faithfully report the correct
///   amount that was read.
pub const READ: Code = Code(0x7f_ff_ff_01);

/// The lock code for write-like operations.
pub const WRITE: Code = Code(0x7f_ff_ff_02);

/// The lock code for I/O control operations.
pub const IO_CTL: Code = Code(0x7f_ff_ff_03);

/// The lock code for named pipe connect operations.
pub const CONNECT_NAMED_PIPE: Code = Code(0x7f_ff_ff_04);

/// A unique code that designates exactly how any one given overlapped result
/// must be treated. This has safety implications, because treating the
/// overlapped results of something like a WRITE as a READ instead could result
/// in assuming that uninitialized memory has been initialized by the write
/// operation.
pub struct Code(pub(crate) u32);

/// The outcome of an overlapped operation.
pub enum OverlappedOutcome {
    /// Do not modify the state of the overlapped header.
    None,
    /// Advance the read/write cursor by the given amount.
    Advance(usize),
}

impl OverlappedOutcome {
    pub(crate) fn apply_to<H, I>(self, guard: &LockGuard<'_, H, I>)
    where
        H: AsRawHandle,
        I: Flavor<H>,
    {
        match self {
            Self::None => (),
            Self::Advance(n) => {
                guard.advance(n);
            }
        }
    }
}

/// The model for a single I/O operation.
///
/// # Safety
///
/// The implementor must assert that the code used is appropriate for the kind
/// of operation and buffer assumptions that it tries to do.
pub unsafe trait Operation<H> {
    /// The output of the operation.
    type Output;

    /// The lock code to use for the operation.
    const CODE: Code;

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
    fn collect(
        &mut self,
        result: OverlappedResult,
        pool: &BufferPool,
    ) -> io::Result<(Self::Output, OverlappedOutcome)>;
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

unsafe impl<H, B> Operation<H> for Write<B>
where
    H: AsRawHandle,
    B: AsRef<[u8]>,
{
    type Output = usize;
    const CODE: Code = WRITE;

    fn prepare(
        &mut self,
        handle: &mut H,
        overlapped: &mut Overlapped,
        pool: &BufferPool,
    ) -> io::Result<()> {
        let buf = self.buf.as_ref();
        let mut b = pool.take(buf.len());
        b.put_slice(buf);
        handle.write_overlapped(b.filled(), overlapped)?;
        Ok(())
    }

    fn collect(
        &mut self,
        result: OverlappedResult,
        _: &BufferPool,
    ) -> io::Result<(Self::Output, OverlappedOutcome)> {
        Ok((result.bytes_transferred, OverlappedOutcome::None))
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

unsafe impl<H, B> Operation<H> for Read<B>
where
    H: AsRawHandle,
    B: AsMut<[u8]>,
{
    type Output = usize;
    const CODE: Code = READ;

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

    fn collect(
        &mut self,
        result: OverlappedResult,
        pool: &BufferPool,
    ) -> io::Result<(Self::Output, OverlappedOutcome)> {
        let b = pool.release(result.bytes_transferred);
        let filled = b.filled();
        self.buf.as_mut()[..filled.len()].copy_from_slice(filled);
        let outcome = OverlappedOutcome::Advance(filled.len());
        Ok((filled.len(), outcome))
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

unsafe impl<H> Operation<H> for ConnectNamedPipe
where
    H: AsRawHandle,
{
    type Output = ();
    const CODE: Code = CONNECT_NAMED_PIPE;

    fn prepare(
        &mut self,
        handle: &mut H,
        overlapped: &mut Overlapped,
        _: &BufferPool,
    ) -> io::Result<Self::Output> {
        handle.connect_overlapped(overlapped)
    }

    fn collect(
        &mut self,
        _: OverlappedResult,
        _: &BufferPool,
    ) -> io::Result<(Self::Output, OverlappedOutcome)> {
        Ok(((), OverlappedOutcome::None))
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

unsafe impl<H, M> Operation<H> for DeviceIoCtl<M>
where
    H: AsRawHandle,
    M: ioctl::Ioctl,
{
    type Output = usize;
    const CODE: Code = IO_CTL;

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
        handle.device_io_control_overlapped(M::CONTROL, Some(buf.filled()), None, overlapped)?;
        Ok(())
    }

    fn collect(
        &mut self,
        result: OverlappedResult,
        _: &BufferPool,
    ) -> io::Result<(Self::Output, OverlappedOutcome)> {
        Ok((result.bytes_transferred, OverlappedOutcome::None))
    }
}
