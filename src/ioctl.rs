/// A trait describing an I/O control operation.
pub trait Ioctl {
    /// Control code for the message.
    const CONTROL: u32;

    /// Length of the structure as a DWORD.
    fn len(&self) -> usize;
}
