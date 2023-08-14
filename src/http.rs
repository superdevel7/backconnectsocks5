use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine};
use std::sync::{atomic::AtomicU64, Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn serve_http(
    mut socket: TcpStream,
    proxy_info: crate::ProxyInfo,
    delay: Arc<AtomicU64>,
) -> Result<(u64, u64)> {
    // encode b64 credentials for m_proxy and tpp proxy
    let b64_m_credentials = general_purpose::STANDARD.encode(
        format!(
            "{}:{}",
            proxy_info.m_credentials.0, proxy_info.m_credentials.1
        )
        .as_bytes(),
    );
    let m_proxy_auth = format!("Basic {b64_m_credentials}");
    let b64_tpp_credentials = general_purpose::STANDARD
        .encode(format!("{}:{}", proxy_info.username, proxy_info.password).as_bytes());
    let tpp_proxy_auth = b64_tpp_credentials.as_bytes().to_vec();
    let proxy_auth = Some((b64_m_credentials, tpp_proxy_auth));

    // peek the socket to search for Proxy-Authorization header
    let mut peek_buffer = vec![0u8; 1024];
    let request_size = socket.peek(&mut peek_buffer).await?;
    let request_str = String::from_utf8_lossy(&peek_buffer);
    let connect = request_str
        .split_once("\r\n\r\n")
        .and_then(|(header, _): (&str, &str)| {
            header
                .split_once("Proxy-Authorization:")
                .map(|(_, auth)| auth)
        })
        .and_then(|auth| {
            if auth.trim_start().starts_with(&m_proxy_auth) {
                Some(())
            } else {
                None
            }
        });
    // if we have the Proxy-Authorization header with correct m_credentials we can connect to tpp proxy
    // we'll change the m_credentials to tpp credentials in pipe module
    match connect {
        Some(_) => {
            // connect with third party proxy
            let mut proxy_socket = TcpStream::connect(proxy_info.proxy_addr)
                .await
                .context("Cannot connect to tpp")?;

            log::debug!("Connected to tpp proxy");

            //pipeline client with tpp for the bidirectional traffic with authorization
            crate::pipe::bidirectional_pipe(&mut socket, &mut proxy_socket, delay, proxy_auth)
                .await
                .context("Failed to relay data")
        } // otherwise we send a 407 Response to the client
        None => {
            let mut read_buffer = vec![0u8; request_size];
            socket.read_exact(&mut read_buffer).await?;
            let response = "HTTP/1.1 407 Proxy Authentication Required\r\n\
                        Proxy-Authenticate: Basic realm=\"Proxy Realm\"\r\n\
                        Content-Length: 0\r\n\r\n";
            socket.write_all(response.as_bytes()).await?;
            Ok((request_size as u64, response.len() as u64))
        }
    }
}
