use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine};
use std::sync::{atomic::AtomicU64, Arc};
use tokio::net::TcpStream;

pub async fn serve_http(
    mut socket: TcpStream,
    proxy_info: crate::ProxyInfo,
    delay: Arc<AtomicU64>,
) -> Result<(u64, u64)> {
    // build the basic authorization header from proxy_info username and password fields
    // this header will be added to the client request in the pipe module (passed as 'auth' to bidirectional_pipe fn)
    let b64_credentials = general_purpose::STANDARD
        .encode(format!("{}:{}", proxy_info.username, proxy_info.password).as_bytes());
    let proxy_auth = format!("\r\nProxy-Authorization: Basic {b64_credentials}")
        .as_bytes()
        .to_vec();

    // connect with third party proxy
    let mut proxy_socket = TcpStream::connect(proxy_info.proxy_addr)
        .await
        .context("Cannot connect to tpp")?;

    log::info!("Connected to tpp proxy");

    //pipeline client with tpp for the bidirectional traffic with authorization
    let (bytes_sent, bytes_received) =
        crate::pipe::bidirectional_pipe(&mut socket, &mut proxy_socket, delay, Some(proxy_auth))
            .await
            .context("Failed to relay data")?;

    Ok((bytes_sent, bytes_received))
}
