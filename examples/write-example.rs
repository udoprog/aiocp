use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use tokio::io::AsyncWriteExt as _;

#[tokio::main]
async fn main() -> io::Result<()> {
    let (port, handle) = async_iocp::setup(2)?;

    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(async_iocp::flags::FILE_FLAG_OVERLAPPED)
        .open("read.txt")?;

    let buf = b"Hello World\n\nBaz";

    let mut io = port.register(output, 33)?;
    io.writer().write_all(&buf[..]).await?;

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
