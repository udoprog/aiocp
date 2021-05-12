use miocp::IoCompletionPort;
use miocp::WriteExt as _;
use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use std::sync::Arc;

#[tokio::main]
async fn main() -> io::Result<()> {
    let port = Arc::new(IoCompletionPort::create(2)?);
    let port2 = port.clone();

    std::thread::spawn(move || loop {
        loop {
            port2.wait().unwrap();
        }
    });

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(miocp::flags::FILE_FLAG_OVERLAPPED)
        .open("read.txt")?;

    port.register_handle(&output)?;

    let pool = miocp::Pool::new();

    let buf = b"Hello World\n";

    let overlapped = miocp::Overlapped::new([buf.len()]);

    unsafe {
        overlapped
            .perform(
                &mut output,
                |output, overlapped, [mut b]| {
                    b.copy_from(buf);
                    output.write_overlapped(b, overlapped)
                },
                |result| Ok(result.bytes_transferred),
            )
            .await?;
    }

    Ok(())
}
