use std::convert::TryFrom as _;
use std::io;
use std::mem;
use std::os::windows::io::AsRawSocket;

use tokio::io::ReadBuf;
use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::Networking::WinSock::{AcceptEx, SOCKET};

use crate::io::Overlapped;
use crate::pool::SocketBuf;

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
            let output_buf_len = u32::try_from(output_buf.len()).expect("output buffer oob");
            let local_address_len =
                u32::try_from(local_address_len).expect("local address length oob");
            let remote_address_len =
                u32::try_from(remote_address_len).expect("local address length oob");
            let mut n = mem::MaybeUninit::uninit();

            let result = AcceptEx(
                self.as_raw_socket() as SOCKET,
                accept.as_raw_socket(),
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
