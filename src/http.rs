use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine};
use std::sync::{atomic::AtomicU64, Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub async fn serve_http(
    mut socket: TcpStream,
    proxy_info: crate::ProxyInfo,
    delay: Arc<AtomicU64>,
) -> Result<(u64, u64)> {
    let b64_credentials = general_purpose::STANDARD
        .encode(format!("{}:{}", proxy_info.username, proxy_info.password).as_bytes());
    let proxy_auth = format!("\r\nProxy-Authorization: Basic {b64_credentials}")
        .as_bytes()
        .to_vec();

    // peek the input stream to get the size of the connect request
    let mut tmp = vec![0u8; 1024];
    let size = socket.peek(&mut tmp).await?;
    let request = String::from_utf8_lossy(&tmp[..size]);
    let header_size = match request.split_once("\r\n\r\n") {
        Some((header, _)) => header.len(),
        None => 0,
    };
    log::info!("Peek: request size={size}");
    drop(tmp);

    // connect with third party proxy
    let mut proxy_socket = TcpStream::connect(proxy_info.proxy_addr)
        .await
        .context("Cannot connect to tpp")?;
    log::info!("Connected to tpp proxy");

    // read the connect request from stream add the Proxy-Authorization buffer and send it to tpp
    let mut buf = vec![0u8; size];
    socket.read_exact(&mut buf).await?;
    buf.splice(header_size..header_size, proxy_auth);
    log::info!("connect request: {:?}", String::from_utf8_lossy(&buf));
    proxy_socket.write_all(&buf).await?;

    //pipeline client with tpp for the rest of the bidirectional traffic
    let (bytes_sent, bytes_received) =
        crate::pipe::bidirectional_pipe(&mut socket, &mut proxy_socket, delay)
            .await
            .context("Failed to relay data")?;

    Ok((bytes_sent, bytes_received))
}
