use std::io;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use winapi::shared::winerror;

#[tokio::main]
async fn main() -> io::Result<()> {
    let (port, handle) = aiocp::setup(2)?;

    let server = aiocp::CreatePipeOptions::new().create(r"\\.\pipe\test")?;
    let client = aiocp::OpenOptions::new().open(r"\\.\pipe\test")?;

    let mut server = port.register(server, 0)?;
    let mut client = port.register(client, 0)?;

    let server = tokio::spawn(async move {
        match server.connect_named_pipe().await {
            Ok(()) => (),
            Err(e) if e.raw_os_error() == Some(winerror::ERROR_PIPE_CONNECTED as i32) => (),
            Err(e) => {
                return Err(e);
            }
        }

        aiocp::tokio::Io::new(&mut server)
            .write_all(b"ping")
            .await?;
        Ok::<_, io::Error>(())
    });

    let client = tokio::spawn(async move {
        let mut buf = [0u8; 4];
        aiocp::tokio::Io::new(&mut client)
            .read_exact(&mut buf)
            .await?;
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
