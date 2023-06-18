// https://github.com/rust-lang/rust-clippy/issues/7271
#![allow(clippy::needless_lifetimes)]

pub mod args;
pub mod errors;
pub mod list;
pub mod socks5;

use crate::args::Args;
use crate::errors::*;
use anyhow::Result;
use std::collections::HashMap;
// use arc_swap::ArcSwap;
/// Importing the `DateTime` type from the `chrono` crate.
// use chrono::prelude::*;
use env_logger::Env;
/// Importing all the traits that are needed to use the mysql crate.
/// Importing all the traits that are needed to use the mysql crate.
use mysql::*;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
// use std::path::PathBuf;
use structopt::StructOpt;
use tokio::net::TcpListener;
// use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
// use tokio::signal::unix::{signal, SignalKind};
use rand::{distributions::Alphanumeric, Rng};
// use tokio::signal;
use dotenv::dotenv;
use std::env;
use warp::http::StatusCode;
use warp::Filter;

#[derive(Deserialize, Serialize)]
pub struct AddProxyBody {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub m_port: u16,
    pub allow_ip: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct CancelProxyBody {
    pub m_port: u16,
}

#[derive(Deserialize, Serialize)]
pub struct BuyResponseData {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub m_username: String,
    pub m_password: String,
}

#[derive(Deserialize, Serialize)]
pub struct StopResponseData {
    pub protocol: String,
    pub host: String,
    pub port: u16,
}

#[derive(Deserialize, Serialize)]
pub struct ErrMessage {
    pub message: String,
}

#[derive(Debug)]
pub struct ProxyHandle {
    cancel_token: CancellationToken,
    // handle: JoinHandle<()>,
}

// lazy_static! {
//     static ref CONN: Mutex<PooledConn> = {
//         let url = "mysql://root:password@127.0.0.1:3306/backconnect";
//         let pool = Pool::new(url).unwrap();
//         let m = pool.get_conn().unwrap();
//         Mutex::new(m)
//     };
// }

pub async fn run_server(
    host: String,
    port: u16,
    m_port: u16,
    username: String,
    password: String,
    allow_ip: Vec<String>,
    proxies: Arc<Mutex<HashMap<u16, ProxyHandle>>>,
) -> Result<(String, String)> {
    let addr: String = format!("{}:{}", host, port).parse()?;

    // let mut data_in: u64 = 0;
    // let mut data_out: u64 = 0;
    let data_in = Arc::new(Mutex::new(0u64));
    let data_out = Arc::new(Mutex::new(0u64));

    let mut m_username: String = "".to_string();
    let mut m_password: String = "".to_string();

    if allow_ip.len() == 0 {
        m_username = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(7)
            .map(char::from)
            .collect();

        m_password = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(15)
            .map(char::from)
            .collect();

        // println!("username: {}", m_username);
        // println!("password: {}", m_password);
    }

    // This is for using in thread
    let response_m_username = m_username.clone();
    let response_m_password = m_password.clone();

    let proxy: SocketAddr = addr.parse().unwrap();

    let bind: SocketAddr = format!("0.0.0.0:{}", m_port).parse().unwrap();

    info!("Binding listener to {}", bind);
    let listener = TcpListener::bind(bind).await?;

    let cancel_token = CancellationToken::new();
    let cancel_token_clone = cancel_token.clone();

    // let handle =
    tokio::spawn(async move {
        let cancel_token_clone = cancel_token.clone();
        loop {
            tokio::select! {
                res = listener.accept() => {
                    let (socket, src) = match res {
                        Ok(x) => x,
                        Err(err) => {
                            error!("Failed to accept connection: {:#}", err);
                            continue;
                        },
                    };
                    debug!("Got new client connection from {}", src);

                    if allow_ip.len() > 0 {

                        if src.is_ipv4() {
                            let ip_str: String = match src.ip() {
                                IpAddr::V4(ip) => format!("{}.{}.{}.{}", ip.octets()[0], ip.octets()[1], ip.octets()[2], ip.octets()[3]),
                                IpAddr::V6(_ip) => "".to_string(),
                            };
                            if!allow_ip.contains(&ip_str) {
                                error!("Invalid IPv4 address: {}", src.ip());
                                continue;
                            }
                        } else {
                            error!("Invalid ip address");
                            continue;
                        }
                    }

                    let m_username = m_username.clone();
                    let m_password = m_password.clone();
                    let username = username.clone();
                    let password = password.clone();
                    let data_in_clone = Arc::clone(&data_in);
                    let data_out_clone = Arc::clone(&data_out);
                    tokio::spawn(async move {
                        match socks5::serve_one(socket, m_username, m_password, proxy.clone(), username, password).await {
                            Ok(res) => {
                                let mut di = data_in_clone.lock().await;
                                let mut do_ = data_out_clone.lock().await;
                                *di += res.0;
                                *do_ += res.1;
                            }
                            Err(err) => {
                                warn!("Error serving client: {:#}", err);
                            }
                        }
                    });
                }
                // _ = signal::ctrl_c() => {
                //     break;
                // }
                _ = cancel_token_clone.cancelled() => {
                    break;
                }
            }
        }
    });

    // periodically report to the server
    let data_in_clone = Arc::clone(&data_in);
    let data_out_clone = Arc::clone(&data_out);
    tokio::spawn(async move {
        // let cancel_token_clone = cancel_token.clone();
        let interval = Duration::from_secs(11);
        let mut last_run = Instant::now() - interval;
        loop {
            let mut di = data_in_clone.lock().await;
            let mut do_ = data_out_clone.lock().await;

            let di_v = *di;
            let do_v = *do_;

            if Instant::now() - last_run >= interval && (di_v > 0 || do_v > 0) {
                // Create a new HTTP client
                let client = reqwest::Client::new();

                // Set the URL to send the POST request to
                let url = "http://127.0.0.1:8000/api/report/socks5";

                // Create a JSON object to send in the body of the POST request
                let json_data = json!({
                    "port": m_port,
                    "data_in": di_v,
                    "data_out": do_v,
                });

                *di = 0;
                *do_ = 0;

                // Send the POST request with the JSON data in the body
                let response = client
                    .post(url)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(json_data.to_string())
                    .send()
                    .await
                    .expect("failed to get response");
                // let response = client
                //     .get(url)
                //     .send()
                //     .await
                //     .expect("failed to get response");

                // Print the response status and body
                println!("Response status: {}", response.status());
                println!(
                    "Response body:\n{}",
                    response.text().await.expect("failed to get payload")
                );

                last_run = Instant::now();
            }
            thread::sleep(Duration::from_secs(1));
        }
    });

    let proxy_handle = ProxyHandle {
        cancel_token: cancel_token_clone,
        // handle,
    };

    let mut proxies = proxies.lock().await;
    if proxies.contains_key(&m_port) {
        return Err(anyhow!("Port {} is already in use", m_port));
    }
    proxies.insert(m_port, proxy_handle);

    Ok((response_m_username, response_m_password))
}

pub async fn handle_run_server(
    (body, proxies): (AddProxyBody, Arc<Mutex<HashMap<u16, ProxyHandle>>>),
) -> Result<impl warp::Reply, Infallible> {
    match run_server(
        body.host.clone(),
        body.port.clone(),
        body.m_port.clone(),
        body.username.clone(),
        body.password.clone(),
        body.allow_ip.clone(),
        proxies,
    )
    .await
    {
        Ok((m_username, m_password)) => {
            let data = BuyResponseData {
                protocol: String::from("socks5"),
                host: env::var("SERVER_PUBLIC_IP").unwrap_or_else(|_| "127.0.0.1".to_string()),
                port: body.m_port,
                m_username,
                m_password,
            };
            Ok(warp::reply::with_status(
                warp::reply::json(&data),
                StatusCode::OK,
            ))
            // Ok(reply);
        } // success
        Err(err) => {
            // fail
            error!("Failed to run server: {:#}", err);
            let data = ErrMessage {
                message: "Failed to run server".to_string(),
            };
            Ok(warp::reply::with_status(
                warp::reply::json(&data),
                StatusCode::BAD_REQUEST,
            ))
        }
    }
}

pub async fn handle_stop_server(
    (body, proxies): (CancelProxyBody, Arc<Mutex<HashMap<u16, ProxyHandle>>>),
) -> Result<impl warp::Reply, Infallible> {
    let port = body.m_port;
    let mut proxies = proxies.lock().await;
    if let Some(handle) = proxies.remove(&port) {
        handle.cancel_token.cancel();
        let data = StopResponseData {
            protocol: String::from("socks5"),
            host: env::var("SERVER_PUBLIC_IP").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: body.m_port,
        };
        Ok(warp::reply::with_status(
            warp::reply::json(&data),
            StatusCode::OK,
        ))
    } else {
        let err_message = ErrMessage {
            message: format!("No proxy server found on port {}", port),
        };
        Ok(warp::reply::with_status(
            warp::reply::json(&err_message),
            StatusCode::BAD_REQUEST,
        ))
    }
}

fn json_add_proxy_body() -> impl Filter<Extract = (AddProxyBody,), Error = warp::Rejection> + Clone
{
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}

fn json_cancel_proxy_body(
) -> impl Filter<Extract = (CancelProxyBody,), Error = warp::Rejection> + Clone {
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}

#[tokio::main]
pub async fn main() -> Result<()> {
    let args = Args::from_args();

    dotenv().ok();

    let logging = match (args.quiet, args.verbose) {
        (true, _) => "warn",
        (false, 0) => "info",
        (false, 1) => "info,backconnectsocks5=debug",
        (false, 2) => "debug",
        (false, _) => "debug,backconnectsocks5=trace",
    };

    env_logger::init_from_env(Env::default().default_filter_or(logging));

    let proxies: Arc<Mutex<HashMap<u16, ProxyHandle>>> = Arc::new(Mutex::new(HashMap::new()));
    let proxies_clone = proxies.clone();
    // let test_proxies = proxies.clone();
    // let test_body: AddProxyBody = AddProxyBody {
    //     host: String::from("ip"),
    //     port: 56250,
    //     m_port: port,
    //     username: String::from(""),
    //     password: String::from(""),
    //     allow_ip: vec![String::from("209.145.59.9")],
    // };

    // handle_run_server((test_body, test_proxies)).await?;

    let api_run_server = warp::post()
        .and(warp::path!("api" / "buy"))
        .and(warp::path::end())
        .and(json_add_proxy_body())
        .map(move |body: AddProxyBody| {
            let proxies = proxies.clone();
            (body, proxies)
        })
        .and_then(handle_run_server);

    let api_stop_server = warp::post()
        .and(warp::path!("api" / "stop"))
        .and(warp::path::end())
        .and(json_cancel_proxy_body())
        .map(move |body: CancelProxyBody| {
            let proxies = proxies_clone.clone();
            (body, proxies)
        })
        .and_then(handle_stop_server);

    let routes = api_run_server.or(api_stop_server);
    // warp::serve(routes).bind_with_graceful_shutdown(args.bind);
    // warp::serve(routes).bind(args.bind);
    warp::serve(routes).run(args.bind).await;

    Ok(())
}
