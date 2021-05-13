use crate::buf::IocpBuf;
use crate::io::Overlapped;
use std::convert::TryFrom as _;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::ptr;
use winapi::shared::minwindef::{DWORD, FALSE};
use winapi::um::fileapi;
use winapi::um::ioapiset;
use winapi::um::namedpipeapi;

/// Windows-specific trait for writing to a HANDLE.
pub trait HandleExt {
    /// Perform an overlapped read over the current I/O object.
    fn read_overlapped(&mut self, buf: &mut IocpBuf, overlapped: Overlapped) -> io::Result<()>;

    /// Perform an overlapped write over the current I/O object.
    fn write_overlapped(&mut self, buf: &IocpBuf, overlapped: Overlapped) -> io::Result<usize>;

    /// Perform an overlapped connect over the current I/O object under the
    /// assumption that it is a named pipe.
    fn connect_overlapped(&mut self, overlapped: Overlapped) -> io::Result<()>;

    /// Execute an I/O device control operation.
    fn device_io_control_overlapped(
        &mut self,
        io_control_code: u32,
        in_buffer: Option<&IocpBuf>,
        out_buffer: Option<&mut IocpBuf>,
        overlapped: Overlapped,
    ) -> io::Result<usize>;
}

impl<O> HandleExt for O
where
    O: AsRawHandle,
{
    fn read_overlapped(&mut self, buf: &mut IocpBuf, mut overlapped: Overlapped) -> io::Result<()> {
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
            Ok(())
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

    fn device_io_control_overlapped(
        &mut self,
        io_control_code: u32,
        in_buffer: Option<&IocpBuf>,
        out_buffer: Option<&mut IocpBuf>,
        mut overlapped: Overlapped,
    ) -> io::Result<usize> {
        unsafe {
            let mut n = mem::MaybeUninit::zeroed();

            let (in_buffer, in_buffer_len) = match in_buffer {
                Some(buf) => {
                    let len = DWORD::try_from(buf.len()).expect("input buffer oob");
                    (buf.as_ptr() as *const _ as *mut _, len)
                }
                None => (ptr::null_mut(), 0),
            };

            let (out_buffer, out_buffer_len) = match out_buffer {
                Some(buf) => {
                    let len = DWORD::try_from(buf.len()).expect("input buffer oob");
                    (buf.as_mut_ptr() as *mut _, len)
                }
                None => (ptr::null_mut(), 0),
            };

            let result = ioapiset::DeviceIoControl(
                self.as_raw_handle() as *mut _,
                io_control_code,
                in_buffer,
                in_buffer_len,
                out_buffer,
                out_buffer_len,
                n.as_mut_ptr(),
                overlapped.as_ptr(),
            );

            if result == FALSE {
                return Err(io::Error::last_os_error());
            }

            let n = usize::try_from(n.assume_init()).expect("output oob");
            Ok(n)
        }
    }
}