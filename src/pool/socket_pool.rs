use std::io;
use std::ptr;
use std::sync::Arc;

use windows_sys::Win32::Networking::WinSock::{
    closesocket, WSASocketW, INVALID_SOCKET, SOCKET, WSAPROTOCOL_INFOW, WSA_FLAG_OVERLAPPED,
};

/// A pointer to the buffer for a socket to receive.
pub struct SocketBuf(pub(crate) SOCKET);

impl SocketBuf {
    /// Access the underlying pointer.
    pub fn as_raw_socket(&mut self) -> SOCKET {
        self.0
    }
}

pub struct SocketPool {
    sockets: Vec<SOCKET>,
    taken: usize,
    released: usize,
    info: Arc<WSAPROTOCOL_INFOW>,
}

impl SocketPool {
    /// Construct a new socket pool.
    pub(crate) fn new(info: Arc<WSAPROTOCOL_INFOW>) -> Self {
        Self {
            sockets: Vec::new(),
            taken: 0,
            released: 0,
            info,
        }
    }

    /// Copy the info associated with this pool.
    pub(crate) fn info(&self) -> Arc<WSAPROTOCOL_INFOW> {
        self.info.clone()
    }

    /// Construct a new socket.
    fn new_socket(&self) -> io::Result<SOCKET> {
        unsafe {
            let result = WSASocketW(
                self.info.iAddressFamily,
                self.info.iSocketType,
                self.info.iProtocol,
                ptr::null_mut(),
                0,
                WSA_FLAG_OVERLAPPED,
            );

            if result == INVALID_SOCKET {
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
            unsafe {
                closesocket(*socket);
            };
        }

        self.released = 0;
        self.taken = 0;
    }
}
