use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[tokio::main]
async fn main() -> io::Result<()> {
    aiocp_examples::init_logging("aiocp=trace");

    let (port, handle) = aiocp::setup(2)?;

    let server = aiocp::pipe::CreatePipeOptions::new()
        .custom_flags(aiocp::flags::FILE_FLAG_OVERLAPPED)
        .create(r"\\.\pipe\test")?;

    let client = OpenOptions::new()
        .custom_flags(aiocp::flags::FILE_FLAG_OVERLAPPED)
        .read(true)
        .write(true)
        .open(r"\\.\pipe\test")?;

    let mut server = port.register(server, 0)?;
    let mut client = port.register(client, 0)?;

    let server = tokio::spawn(async move {
        server.write_all(b"ping").await?;
        Ok::<_, io::Error>(())
    });

    let client = tokio::spawn(async move {
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await?;
        Ok::<_, io::Error>(buf)
    });

    let (server, client) = tokio::try_join!(server, client)?;
    let () = server?;
    let buf = client?;

    assert_eq!(&buf[..], b"ping");

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
