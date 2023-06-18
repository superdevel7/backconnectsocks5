// https://github.com/rust-lang/rust-clippy/issues/7271
#![allow(clippy::needless_lifetimes)]

pub mod args;
//pub mod errors;
pub mod admin;
pub mod pipe;
pub mod socks5;

use crate::args::Args;
use anyhow::Result;
use env_logger::Env;
use log::{debug, error, info};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::ops::{Add, AddAssign};
use std::{
    env,
    sync::{
        atomic::{AtomicU64, AtomicUsize},
        Arc,
    },
};
use structopt::StructOpt;
use tokio::net::TcpListener;
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedSender},
    RwLock,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use warp::{http::StatusCode, Filter};

use admin::{handle_admin_endpoint, start_admin_thread};

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
pub struct AdminProxyBody {
    pub m_port: u16,
    pub delay: u64,
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

#[derive(Debug, Clone, Copy)]
pub struct Traffic(u64, u64);

impl Add for Traffic {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Traffic(self.0 + rhs.0, self.1 + rhs.1)
    }
}
impl AddAssign for Traffic {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
        self.1 += rhs.1;
    }
}

#[derive(Debug)]
pub struct ProxyHandle {
    cancel_token: CancellationToken,
    traffic: Traffic,
    delay: Arc<AtomicU64>,
}

#[derive(Debug)]
pub enum ThreadMessage {
    StartProxy(u16, Arc<AtomicU64>),
    NewServeOne(u16, JoinHandle<Option<(u64, u64)>>),
}

pub async fn run_server(
    host: String,
    port: u16,
    m_port: u16,
    username: String,
    password: String,
    allow_ip: Vec<String>,
    proxies: Arc<RwLock<HashMap<u16, ProxyHandle>>>,
    tx: UnboundedSender<ThreadMessage>,
) -> Result<(String, String)> {
    let m_credentials = if allow_ip.is_empty() {
        (
            rand::thread_rng()
                .sample_iter(&Alphanumeric)
                .take(7)
                .map(char::from)
                .collect(),
            rand::thread_rng()
                .sample_iter(&Alphanumeric)
                .take(15)
                .map(char::from)
                .collect(),
        )
    } else {
        (String::new(), String::new())
    };

    let m_credentials_clone = m_credentials.clone();

    let proxy_addr: SocketAddr = format!("{}:{}", host, port).parse()?;

    let bind: SocketAddr = format!("0.0.0.0:{}", m_port).parse()?;

    let cancel_token = CancellationToken::new();
    let cancel_token_clone = cancel_token.clone();
    let proxy_handle = ProxyHandle {
        cancel_token,
        traffic: Traffic(0, 0),
        delay: Arc::new(0.into()),
    };
    info!("Binding listener to {}", bind);
    let listener = TcpListener::bind(bind).await?;
    let mut proxies_hashmap = proxies.write().await;
    proxies_hashmap
        .entry(m_port)
        .and_modify(|handle| {
            handle.cancel_token.cancel();
            handle.cancel_token = proxy_handle.cancel_token.clone();
            handle.traffic = Traffic(0, 0);
            handle.delay = Arc::new(0.into());
        })
        .or_insert(proxy_handle);
    // anyhow::ensure!(!proxies_hashmap.contains_key(&m_port), "Port {} is already in use", m_port );
    // proxies_hashmap.insert(m_port, proxy_handle);

    tokio::spawn(async move {
        // create an AtomicU64 which will control the transfer speed for this proxy.
        // send the Arc<AtomicU64> to the admin_thread so it can be stored in the proxies hashmap and controled in real time by admin
        let atomic_delay = Arc::new(AtomicU64::new(0));
        match tx.send(ThreadMessage::StartProxy(m_port, atomic_delay.clone())) {
            Ok(_) => debug!(
                "start proxy on port {} message sent to admin_thread",
                m_port
            ),
            Err(err) => error!("Error on sending start proxy message {:?}", err),
        }
        loop {
            info!("proxy listen on {}", m_port);
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

                    if !allow_ip.is_empty(){
                        if src.is_ipv4() {
                            let ip_str: String = match src.ip() {
                                IpAddr::V4(ip) => format!("{}.{}.{}.{}", ip.octets()[0], ip.octets()[1], ip.octets()[2], ip.octets()[3]),
                                IpAddr::V6(_ip) => "".to_string(),
                            };
                            if !allow_ip.contains(&ip_str) {
                                error!("Invalid IPv4 address: {}", src.ip());
                                continue;
                            }
                        } else {
                            error!("Invalid ip address");
                            continue;
                        }
                    }
                    let delay = atomic_delay.clone();
                    let m_credentials = m_credentials_clone.clone();
                    let username = username.clone();
                    let password = password.clone();
                    let s1_handle = tokio::spawn(async move {
                        debug!("start serve_one thread");
                        match socks5::serve_one(socket, m_credentials, proxy_addr, username, password, delay).await {
                            Ok(traffic) => {
                                info!("end serve_one thread with traffic: {:?}", traffic);
                                Some(traffic)
                            }
                            Err(err) => {
                                error!("Error serve_one {} : {:?}", m_port, err);
                                None
                            }
                        }
                    });
                    //after the start of serve_one we send the s1_handle to the admin thread to monitor it
                    match tx.send(ThreadMessage::NewServeOne(m_port, s1_handle)) {
                        Ok(_) => debug!("join_handle sent"),
                        Err(err) => error!("Error on sending join_handle {:?}", err)
                    }
                }
                _ = cancel_token_clone.cancelled() => break
            }
        }
    });

    Ok(m_credentials)
}

pub async fn handle_run_server(
    (body, proxies, tx): (
        AddProxyBody,
        Arc<RwLock<HashMap<u16, ProxyHandle>>>,
        UnboundedSender<ThreadMessage>,
    ),
) -> Result<impl warp::Reply, Infallible> {
    match run_server(
        body.host.clone(),
        body.port.clone(),
        body.m_port.clone(),
        body.username.clone(),
        body.password.clone(),
        body.allow_ip.clone(),
        proxies,
        tx,
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
    (body, proxies): (CancelProxyBody, Arc<RwLock<HashMap<u16, ProxyHandle>>>),
) -> Result<impl warp::Reply, Infallible> {
    let port = body.m_port;
    let mut proxies = proxies.write().await;
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

fn json_admin_proxy_body(
) -> impl Filter<Extract = (AdminProxyBody,), Error = warp::Rejection> + Clone {
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}

fn json_cancel_proxy_body(
) -> impl Filter<Extract = (CancelProxyBody,), Error = warp::Rejection> + Clone {
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}

#[tokio::main]
pub async fn main() -> Result<()> {
    let args = Args::from_args();

    dotenv::dotenv().ok();

    let logging = match (args.quiet, args.verbose) {
        (true, _) => "warn",
        (false, 0) => "info",
        (false, 1) => "info,backconnectsocks5=debug",
        (false, 2) => "debug",
        (false, _) => "debug,backconnectsocks5=trace",
    };

    env_logger::init_from_env(Env::default().default_filter_or(logging));

    let proxies: Arc<RwLock<HashMap<u16, ProxyHandle>>> = Arc::new(RwLock::new(HashMap::new()));
    let proxies_clone_stop = proxies.clone();
    let proxies_clone_admin = proxies.clone();
    let thread_counter = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = unbounded_channel::<ThreadMessage>();

    start_admin_thread(rx, proxies.clone(), thread_counter.clone()).await?;

    let api_run_admin = warp::post()
        .and(warp::path!("api" / "admin"))
        .and(warp::path::end())
        .and(json_admin_proxy_body())
        .map(move |body: AdminProxyBody| {
            (body, proxies_clone_admin.clone(), thread_counter.clone())
        })
        .and_then(handle_admin_endpoint);

    let api_run_server = warp::post()
        .and(warp::path!("api" / "buy"))
        .and(warp::path::end())
        .and(json_add_proxy_body())
        .map(move |body: AddProxyBody| (body, proxies.clone(), tx.clone()))
        .and_then(handle_run_server);

    let api_stop_server = warp::post()
        .and(warp::path!("api" / "stop"))
        .and(warp::path::end())
        .and(json_cancel_proxy_body())
        .map(move |body: CancelProxyBody| (body, proxies_clone_stop.clone()))
        .and_then(handle_stop_server);

    let routes = api_run_admin.or(api_run_server).or(api_stop_server);
    warp::serve(routes).run(args.bind).await;

    Ok(())
}
