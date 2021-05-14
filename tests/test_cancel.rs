use std::fs::OpenOptions;
use std::future::Future as _;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use std::task::Poll;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[tokio::test]
async fn test_cancel() -> io::Result<()> {
    let (port, handle) = aiocp::setup(2)?;

    let server = aiocp::CreatePipeOptions::new().create(r"\\.\pipe\test-cancel")?;
    let client = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(aiocp::flags::FILE_FLAG_OVERLAPPED)
        .open(r"\\.\pipe\test-cancel")?;

    let mut client = port.register(client, 0)?;

    // NB: cancel a client read.
    {
        let mut buf = [1u8; 128];
        let future = client.read(&mut buf);
        tokio::pin!(future);

        let mut once = true;

        futures::future::poll_fn(|cx| {
            if !std::mem::take(&mut once) {
                return Poll::Ready(Ok(0));
            }

            cx.waker().wake_by_ref();
            future.as_mut().poll(cx)
        })
        .await?;
    }

    let mut server = port.register(server, 0)?;

    // NB: try to make use of the thing after having a cancelled operation.
    /*
    let mut buf = [0u8; 4];

    let mut server_io = aiocp::tokio::Io::new(&mut server);
    let mut client_io = aiocp::tokio::Io::new(&mut client);

    let server_read = server_io.read_exact(&mut buf);
    let client_write = client_io.write_all(b"ping");

    println!("blocking...");

    let (read, ()) = tokio::try_join!(server_read, client_write)?;
    assert_eq!(read, 4);
    assert_eq!(&buf[..], b"ping");

    println!("shutting down...");
    */

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
