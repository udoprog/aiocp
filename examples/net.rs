use std::io;
use std::net;

#[tokio::main]
async fn main() -> io::Result<()> {
    iocp_examples::init_logging();
    let (port, handle) = iocp::setup(1)?;

    let socket = net::TcpListener::bind("127.0.0.1:12345")?;

    let mut socket = port.register_socket(socket, Default::default())?;
    let _client = socket.accept().await?;

    port.shutdown()?;
    handle.join()?;
    Ok(())
}
