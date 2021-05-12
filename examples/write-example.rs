use async_iocp::CompletionPort;
use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use std::sync::Arc;

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

    let output = OpenOptions::new()
        .write(true)
        .custom_flags(async_iocp::flags::FILE_FLAG_OVERLAPPED)
        .open("read.txt")?;

    let buf = b"Hello World\n\nBaz";

    let mut io = port.register(output, 33)?;
    io.write(&buf[..]).await?;
    Ok(())
}
