use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use tokio::io::AsyncReadExt as _;

#[tokio::main]
async fn main() -> io::Result<()> {
    iocp_examples::init_logging();

    let mut it = std::env::args_os();
    it.next();
    let path = it.next().expect("missing <path> argument");

    let (port, handle) = iocp::setup(1)?;

    let output = OpenOptions::new()
        .read(true)
        .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
        .open(path)?;

    let mut io = port.register_handle(output, Default::default())?;
    let mut buf = Vec::new();
    io.read_to_end(&mut buf).await?;

    dbg!(buf.len());

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
