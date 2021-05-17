use crate::io::Overlapped;
use crate::pool::SocketBuf;
use crate::sys::AsRawSocket;
use std::convert::TryFrom as _;
use std::io;
use std::mem;
use tokio::io::ReadBuf;
use winapi::shared::minwindef::{DWORD, FALSE};
use winapi::um::mswsock;
use winapi::um::winsock2;

/// Windows-specific trait for writing to a HANDLE.
pub trait SocketExt {
    /// Perform an overlapped accept.
    fn accept(
        &mut self,
        accept: &mut SocketBuf,
        output_buf: &mut ReadBuf<'_>,
        local_address_len: usize,
        remote_address_len: usize,
        overlapped: &mut Overlapped,
    ) -> io::Result<usize>;
}

impl<S> SocketExt for S
where
    S: AsRawSocket,
{
    /// Perform an overlapped accept.
    fn accept(
        &mut self,
        accept: &mut SocketBuf,
        output_buf: &mut ReadBuf<'_>,
        local_address_len: usize,
        remote_address_len: usize,
        overlapped: &mut Overlapped,
    ) -> io::Result<usize> {
        unsafe {
            let output_buf = output_buf.unfilled_mut();
            let output_buf_len = DWORD::try_from(output_buf.len()).expect("output buffer oob");
            let local_address_len =
                DWORD::try_from(local_address_len).expect("local address length oob");
            let remote_address_len =
                DWORD::try_from(remote_address_len).expect("local address length oob");
            let mut n = mem::MaybeUninit::uninit();

            let result = mswsock::AcceptEx(
                self.as_raw_socket() as winsock2::SOCKET,
                accept.as_mut_ptr() as winsock2::SOCKET,
                output_buf.as_mut_ptr() as *mut _,
                output_buf_len,
                local_address_len,
                remote_address_len,
                n.as_mut_ptr(),
                overlapped.as_ptr() as *mut _,
            );

            if result == FALSE {
                return Err(io::Error::last_os_error());
            }

            let n = n.assume_init();
            Ok(usize::try_from(n).expect("output len overflow"))
        }
    }
}
