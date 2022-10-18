//! Abstractions for building raw overlapping operation helpers.

use crate::completion_port::CompletionPort;
use crate::io::{Code, Overlapped, OverlappedResult};
use crate::pool::{BufferPool, SocketBuf, SocketPool};
use crate::socket::ext::SocketExt as _;
use crate::socket::{LockGuard, Socket};
use std::io;
use std::os::windows::io::{AsRawSocket, FromRawSocket};
use std::os::windows::raw::SOCKET;

/// The lock code for accepting connections.
pub const ACCEPT: Code = Code(0x7f_ff_ff_11);

/// The outcome of an overlapped operation.
pub enum OverlappedOutcome {
    /// Do not modify the state of the overlapped header.
    None,
    /// Advance the read/write cursor by the given amount.
    Advance(usize),
}

impl OverlappedOutcome {
    pub(crate) fn apply_to<S>(self, guard: &mut LockGuard<'_, S>)
    where
        S: AsRawSocket,
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
        pool: &mut BufferPool,
        sockets: &mut SocketPool,
    ) -> io::Result<()>;

    /// Collect the overlapped result into the output of the I/O operation.
    fn collect(
        &mut self,
        result: OverlappedResult,
        port: &CompletionPort,
        pool: &mut BufferPool,
        sockets: &mut SocketPool,
    ) -> io::Result<(Self::Output, OverlappedOutcome)>;
}

pub struct Accept(());

impl Accept {
    /// Construct a new handler for a accept operations.
    pub fn new() -> Self {
        Self(())
    }
}

unsafe impl<S> Operation<S> for Accept
where
    S: AsRawSocket + FromRawSocket,
{
    type Output = Socket<S>;
    const CODE: Code = ACCEPT;

    fn prepare(
        &mut self,
        socket: &mut S,
        overlapped: &mut Overlapped,
        pool: &mut BufferPool,
        sockets: &mut SocketPool,
    ) -> io::Result<()> {
        let mut accept = sockets.take()?;
        let mut output_buf = pool.take(128);
        socket.accept(&mut accept, &mut output_buf, 16, 16, overlapped)?;
        Ok(())
    }

    fn collect(
        &mut self,
        _result: OverlappedResult,
        port: &CompletionPort,
        pool: &mut BufferPool,
        sockets: &mut SocketPool,
    ) -> io::Result<(Self::Output, OverlappedOutcome)> {
        let SocketBuf(accept) = sockets.release();
        let socket = unsafe { S::from_raw_socket(accept as SOCKET) };
        let socket = Socket::new(socket, port.clone(), pool.max_buffer_size())?;
        Ok((socket, OverlappedOutcome::None))
    }
}
