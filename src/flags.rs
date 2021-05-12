/// The file or device is being opened or created for asynchronous I/O.
///
/// When subsequent I/O operations are completed on this handle, the event
/// specified in the OVERLAPPED structure will be set to the signaled state.
pub const FILE_FLAG_OVERLAPPED: u32 = 0x40000000;
