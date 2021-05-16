# iocp

Experimental asynchronous I/O Completion Port driver for Windows.

This is an attempt to build a safe and sound async driver with minimal overhead
around completion ports.

## Limitations

* Each wrapped resource can only have one pending I/O operation at a time.

# Example

```rust
use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;

#[tokio::main]
async fn main() -> io::Result<()> {
    let (port, background) = iocp::setup(1)?;

    let output = OpenOptions::new()
        .read(true)
        .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
        .open("read.txt")?;

    let mut io = port.register(output, 33)?;
    let mut buf = [1u8; 128];
    let n = io.read(&mut buf).await?;
    dbg!(std::str::from_utf8(&buf[..n]).unwrap());

    port.shutdown()?;
    background.join()?;
    Ok(())
}
```
