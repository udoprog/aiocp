use crate::io::Internal;
use crate::sys::AsRawSocket;
use winapi::ctypes::c_void;

#[derive(Clone, Copy)]
struct Flavor;

impl<S> crate::io::Flavor<S> for Flavor
where
    S: AsRawSocket,
{
    fn into_handle(socket: &S) -> *mut c_void {
        socket.as_raw_socket() as *mut _
    }
}

/// A wrapped socket able to perform overlapped operations.
pub struct Socket<S>
where
    S: AsRawSocket,
{
    internal: Internal<S, Flavor>,
}

impl<S> Socket<S>
where
    S: AsRawSocket,
{
    /// Construct a socket wrapper capable of overlapped operations.
    pub(crate) fn new(socket: S, port: crate::sys::CompletionPort, max_buffer_size: usize) -> Self {
        Self {
            internal: Internal::new(socket, port, max_buffer_size),
        }
    }

    /// Duplicate this socket, allowing more than one pending I/O operation to
    /// happen over it concurrently.
    ///
    /// The underlying socket must implement [Clone], which it can trivially do
    /// if it's wrapped in something like an [Arc].
    ///
    /// The returned file socket does not inherit the state of the socket it was
    /// cloned from. It has no pending operations. It also doesn't cause any
    /// racing with the current socket and is free to perform other kinds of
    /// operations.
    pub fn duplicate(&self) -> Self
    where
        S: Clone,
    {
        Self {
            internal: self.internal.duplicate(),
        }
    }

    /// Access a reference to the underlying socket.
    pub fn as_ref(&self) -> &S {
        self.internal.as_ref()
    }

    /// Access a mutable reference to the underlying socket.
    pub fn as_mut(&mut self) -> &mut S {
        self.internal.as_mut()
    }
}

unsafe impl<S> Send for Socket<S> where S: AsRawSocket + Send {}
unsafe impl<S> Sync for Socket<S> where S: AsRawSocket + Sync {}
