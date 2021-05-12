use crate::buf::RawBuf;
use crate::overlapped::RawOverlapped;
use std::convert::TryFrom as _;
use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use winapi::shared::minwindef::{DWORD, FALSE};
use winapi::um::fileapi;

/// Windows-specific trait for writing to a file which is implemented by an
/// underlying HANDLE.
pub trait WriteExt {
    /// Perform an overlapped write over the file.
    unsafe fn write_overlapped(
        &mut self,
        buf: RawBuf,
        overlapped: RawOverlapped,
    ) -> io::Result<usize>;
}

impl<O> WriteExt for O
where
    O: AsRawHandle,
{
    unsafe fn write_overlapped(
        &mut self,
        RawBuf { ptr, len, .. }: RawBuf,
        RawOverlapped(overlapped): RawOverlapped,
    ) -> io::Result<usize> {
        let len = DWORD::try_from(len).unwrap_or(DWORD::MAX);
        let mut n = mem::MaybeUninit::zeroed();

        let result = fileapi::WriteFile(
            self.as_raw_handle(),
            ptr as *const _,
            len,
            n.as_mut_ptr(),
            overlapped,
        );

        if result == FALSE {
            return Err(io::Error::last_os_error());
        }

        let n = usize::try_from(n.assume_init()).expect("written count out-of-bounds");
        Ok(n)
    }
}
