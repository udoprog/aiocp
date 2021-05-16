use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use tokio::io::AsyncWriteExt as _;

#[tokio::main]
async fn main() -> io::Result<()> {
    use tracing_subscriber::prelude::*;

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("trace"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_level(true)
                .compact(),
        )
        .try_init()
        .expect("error initializing logging");

    let (port, handle) = aiocp::setup(2)?;

    let mut it = std::env::args_os();
    it.next();
    let path = it.next().expect("missing <path> argument");

    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(aiocp::flags::FILE_FLAG_OVERLAPPED)
        .open(path)?;

    let buf = b"Hello World\n\nBaz";

    let mut io = port.register(output, 33)?;
    io.write_all(&buf[..]).await?;

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
