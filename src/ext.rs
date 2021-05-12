use crate::buf::IocpBuf;
use crate::io::Overlapped;
use std::convert::TryFrom as _;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use winapi::shared::minwindef::{DWORD, FALSE};
use winapi::um::fileapi;
use winapi::um::namedpipeapi;

/// Windows-specific trait for writing to a HANDLE.
pub trait HandleExt {
    /// Perform an overlapped read over the current I/O object.
    fn read_overlapped(&mut self, buf: &mut IocpBuf, overlapped: Overlapped) -> io::Result<usize>;

    /// Perform an overlapped write over the current I/O object.
    fn write_overlapped(&mut self, buf: &IocpBuf, overlapped: Overlapped) -> io::Result<usize>;

    /// Perform an overlapped connect over the current I/O object, under the
    /// assumption that it is a named pipe.
    fn connect_overlapped(&mut self, overlapped: Overlapped) -> io::Result<()>;
}

impl<O> HandleExt for O
where
    O: AsRawHandle,
{
    fn read_overlapped(
        &mut self,
        buf: &mut IocpBuf,
        mut overlapped: Overlapped,
    ) -> io::Result<usize> {
        unsafe {
            let len = DWORD::try_from(buf.len()).unwrap_or(DWORD::MAX);
            let mut n = mem::MaybeUninit::zeroed();

            let result = fileapi::ReadFile(
                self.as_raw_handle() as *mut _,
                buf.as_mut_ptr() as *mut _,
                len,
                n.as_mut_ptr(),
                overlapped.as_ptr(),
            );

            if result == FALSE {
                return Err(io::Error::last_os_error());
            }

            let n = usize::try_from(n.assume_init()).expect("read count oob");
            buf.set_len(n);
            Ok(n)
        }
    }

    fn write_overlapped(&mut self, buf: &IocpBuf, mut overlapped: Overlapped) -> io::Result<usize> {
        unsafe {
            let len = DWORD::try_from(buf.len()).unwrap_or(DWORD::MAX);
            let mut n = mem::MaybeUninit::zeroed();

            let result = fileapi::WriteFile(
                self.as_raw_handle() as *mut _,
                buf.as_ptr() as *const _,
                len,
                n.as_mut_ptr(),
                overlapped.as_ptr(),
            );

            if result == FALSE {
                return Err(io::Error::last_os_error());
            }

            let n = usize::try_from(n.assume_init()).expect("written count oob");
            Ok(n)
        }
    }

    fn connect_overlapped(&mut self, mut overlapped: Overlapped) -> io::Result<()> {
        unsafe {
            let result =
                namedpipeapi::ConnectNamedPipe(self.as_raw_handle() as *mut _, overlapped.as_ptr());

            if result == FALSE {
                return Err(io::Error::last_os_error());
            }

            Ok(())
        }
    }
}
