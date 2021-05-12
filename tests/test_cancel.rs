use async_iocp::CompletionPort;
use std::fs::OpenOptions;
use std::future::Future as _;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use std::sync::Arc;
use std::task::Poll;

#[tokio::test]
async fn test_cancel() -> io::Result<()> {
    let port = Arc::new(CompletionPort::create(2)?);
    let port2 = port.clone();

    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));

        let status = port2.get_queued_completion_status()?;

        status.header.release();
        Ok::<_, io::Error>(())
    });

    let output = OpenOptions::new()
        .read(true)
        .custom_flags(async_iocp::flags::FILE_FLAG_OVERLAPPED)
        .open("read.txt")?;

    let mut io = port.register(output, 33)?;

    let mut future = Box::pin(async move {
        let mut buf = [1u8; 128];
        let n = io.read(&mut buf).await?;
        dbg!(std::str::from_utf8(&buf[..n]).unwrap());
        Ok::<_, io::Error>(())
    });

    let mut once = true;

    let n = futures::future::poll_fn(move |cx| {
        if std::mem::take(&mut once) {
            cx.waker().wake_by_ref();
            future.as_mut().poll(cx)
        } else {
            Poll::Ready(Ok(()))
        }
    })
    .await?;

    t.join().unwrap()?;
    Ok(())
}
