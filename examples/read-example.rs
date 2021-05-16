use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use tokio::io::AsyncReadExt as _;

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
        .read(true)
        .custom_flags(aiocp::flags::FILE_FLAG_OVERLAPPED)
        .open(path)?;

    let mut io = port.register(output, 0)?;
    let mut buf = Vec::new();
    io.read_to_end(&mut buf).await?;

    dbg!(buf.len());

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
