use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;

#[tokio::main]
async fn main() -> io::Result<()> {
    let (port, handle) = async_iocp::setup(2)?;

    let output = OpenOptions::new()
        .read(true)
        .custom_flags(async_iocp::flags::FILE_FLAG_OVERLAPPED)
        .open("read.txt")?;

    let mut io = port.register(output, 33)?;
    let mut buf = [1u8; 128];
    let n = io.read(&mut buf).await?;
    dbg!(std::str::from_utf8(&buf[..n]).unwrap());

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
