use aiocp::ArcHandle;
use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use tokio::io::AsyncReadExt as _;

#[tokio::main]
async fn main() -> io::Result<()> {
    aiocp_examples::init_logging("aiocp=trace");

    let mut it = std::env::args_os();
    it.next();
    let path = it.next().expect("missing <path> argument");

    let (port, handle) = aiocp::setup(2)?;

    let output = OpenOptions::new()
        .read(true)
        .custom_flags(aiocp::flags::FILE_FLAG_OVERLAPPED)
        .open(path)?;

    let output = ArcHandle::new(output);

    let mut reader1 = port.register(output, 0)?;
    let mut reader2 = reader1.duplicate();

    let mut buf1 = Vec::new();
    let read1 = reader1.read_to_end(&mut buf1);

    let mut buf2 = Vec::new();
    let read2 = reader2.read_to_end(&mut buf2);

    let (n1, n2) = tokio::try_join!(read1, read2)?;
    assert_eq!(n1, n2);
    dbg!(buf1.len(), buf2.len());
    assert_eq!(buf1, buf2);

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
