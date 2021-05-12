use async_iocp::CompletionPort;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use winapi::shared::winerror;

#[tokio::main]
async fn main() -> io::Result<()> {
    let port = Arc::new(CompletionPort::create(2)?);
    let port2 = port.clone();

    std::thread::spawn(move || loop {
        loop {
            let status = port2
                .get_queued_completion_status()
                .expect("failed to get next status");

            status.header.release();
        }
    });

    let server = async_iocp::CreatePipeOptions::new().create(r"\\.\pipe\test")?;
    let client = async_iocp::CreatePipeClientOptions::new().create(r"\\.\pipe\test")?;

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

        server.writer().write_all(b"ping").await?;
        Ok::<_, io::Error>(())
    });

    let client = tokio::spawn(async move {
        let mut buf = [0u8; 4];
        client.reader().read_exact(&mut buf).await?;
        Ok::<_, io::Error>(buf)
    });

    let (server, client) = tokio::try_join!(server, client)?;
    let () = server?;
    let buf = client?;

    assert_eq!(&buf[..], b"ping");
    Ok(())
}
