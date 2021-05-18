use std::io;
use std::ptr;
use std::sync::Arc;
use winapi::um::winsock2;

/// A pointer to the buffer for a socket to receive.
pub struct SocketBuf(pub(crate) winsock2::SOCKET);

impl SocketBuf {
    /// Access the underlying pointer.
    pub fn as_raw_socket(&mut self) -> winsock2::SOCKET {
        self.0
    }
}

pub struct SocketPool {
    sockets: Vec<winsock2::SOCKET>,
    taken: usize,
    released: usize,
    info: Arc<winsock2::WSAPROTOCOL_INFOW>,
}

impl SocketPool {
    /// Construct a new socket pool.
    pub(crate) fn new(info: Arc<winsock2::WSAPROTOCOL_INFOW>) -> Self {
        Self {
            sockets: Vec::new(),
            taken: 0,
            released: 0,
            info,
        }
    }

    /// Copy the info associated with this pool.
    pub(crate) fn info(&self) -> Arc<winsock2::WSAPROTOCOL_INFOW> {
        self.info.clone()
    }

    /// Construct a new socket.
    fn new_socket(&self) -> io::Result<winsock2::SOCKET> {
        unsafe {
            let result = winsock2::WSASocketW(
                self.info.iAddressFamily,
                self.info.iSocketType,
                self.info.iProtocol,
                ptr::null_mut(),
                0,
                winsock2::WSA_FLAG_OVERLAPPED,
            );

            if result == winsock2::INVALID_SOCKET {
                return Err(io::Error::last_os_error());
            }

            Ok(result)
        }
    }

    /// Take a socket from the pool.
    pub(crate) fn take(&mut self) -> io::Result<SocketBuf> {
        if self.sockets.len() <= self.taken {
            self.sockets.push(self.new_socket()?);
        }

        let taken = self.taken;
        self.taken = taken + 1;
        Ok(SocketBuf(self.sockets[taken]))
    }

    /// Release a socket from the pool without freeing it.
    pub(crate) fn release(&mut self) -> SocketBuf {
        let released = self.released;
        self.released = released + 1;
        SocketBuf(self.sockets[released])
    }

    /// Clear all sockets between released to taken.
    pub(crate) fn clear(&mut self) {
        for socket in &self.sockets[self.released..self.taken] {
            let _ = unsafe {
                winsock2::closesocket(*socket);
            };
        }

        self.released = 0;
        self.taken = 0;
    }
}
