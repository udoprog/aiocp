use std::future::Future as _;
use std::io;
use std::task::Poll;

#[tokio::test]
async fn test_cancel() -> io::Result<()> {
    async_iocp::setup_logging().unwrap();
    let (port, handle) = async_iocp::setup(2)?;

    let server = async_iocp::CreatePipeOptions::new().create(r"\\.\pipe\test-cancel")?;
    let client = async_iocp::OpenOptions::new().open(r"\\.\pipe\test-cancel")?;

    let mut io = port.register(client, 33)?;

    {
        let mut buf = [1u8; 128];
        let future = io.read(&mut buf);
        tokio::pin!(future);

        let mut once = true;

        futures::future::poll_fn(|cx| {
            if std::mem::take(&mut once) {
                cx.waker().wake_by_ref();
                future.as_mut().poll(cx)
            } else {
                Poll::Ready(Ok(()))
            }
        })
        .await?;
    }

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
