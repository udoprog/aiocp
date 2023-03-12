mod sealed {
    pub trait Sealed {}
}

/// A trait describing an I/O control operation.
#[allow(clippy::len_without_is_empty)]
pub trait Ioctl: self::sealed::Sealed {
    /// Control code for the message.
    const CONTROL: u32;

    /// Length of the structure as a DWORD.
    fn len(&self) -> usize;
}
