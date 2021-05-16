use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use tokio::io::AsyncWriteExt as _;

#[tokio::main]
async fn main() -> io::Result<()> {
    iocp_examples::init_logging("iocp=trace");

    let (port, handle) = iocp::setup(1)?;

    let mut it = std::env::args_os();
    it.next();
    let path = it.next().expect("missing <path> argument");

    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
        .open(path)?;

    let buf = b"Hello World\n\nBaz";

    let mut io = port.register(output, 33)?;
    io.write_all(&buf[..]).await?;

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
