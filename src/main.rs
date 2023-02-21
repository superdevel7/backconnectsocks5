// https://github.com/rust-lang/rust-clippy/issues/7271
#![allow(clippy::needless_lifetimes)]

pub mod args;
pub mod errors;
pub mod list;
pub mod socks5;

use crate::args::Args;
use crate::errors::*;
use arc_swap::ArcSwap;
use env_logger::Env;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use structopt::StructOpt;
use tokio::net::TcpListener;
use tokio::signal::unix::{signal, SignalKind};
use warp::Filter;

#[derive(Deserialize, Serialize)]
pub struct Body {
    pub country: String,
}

async fn run_server() -> Result<()> {
    let addr: String = format!("0.0.0.0:{}", 3333).parse()?;

    let proxy_list: PathBuf = "/path/to/proxy/list".parse().unwrap();
    let bind: SocketAddr = addr.parse().unwrap();

    // a stream of sighup signals
    let mut sighup = signal(SignalKind::hangup())?;

    let proxies = list::load_from_path(&proxy_list)
        .await
        .context("Failed to load proxy list")?;
    let proxies = ArcSwap::from(Arc::new(proxies));

    info!("Binding listener to {}", bind);
    let listener = TcpListener::bind(bind).await?;

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
                let proxies = proxies.load();
                tokio::spawn(async move {
                    if let Err(err) = socks5::serve(socket, proxies.clone()).await {
                        warn!("Error serving client: {:#}", err);
                    }
                });
            }
            _ = sighup.recv() => {
                debug!("Got signal HUP");
                match list::load_from_path(&proxy_list).await {
                    Ok(list) => {
                        let list = Arc::new(list);
                        proxies.store(list);
                    }
                    Err(err) => {
                        error!("Failed to reload proxy list: {:#}", err);
                    }
                }
            }
        }
    }
    Ok(())
}

async fn handle_request(body: Body) -> Result<impl warp::Reply, Infallible> {
    match run_server().await {
        Ok(_) => Ok(warp::reply::reply()), // success
        Err(err) => {
            // fail
            error!("Failed to run server: {:#}", err);
            Ok(warp::reply::reply())
        }
    }
}

fn json_body() -> impl Filter<Extract = (Body,), Error = warp::Rejection> + Clone {
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::from_args();

    let logging = match (args.quiet, args.verbose) {
        (true, _) => "warn",
        (false, 0) => "info",
        (false, 1) => "info,backconnectsocks5=debug",
        (false, 2) => "debug",
        (false, _) => "debug,backconnectsocks5=trace",
    };
    env_logger::init_from_env(Env::default().default_filter_or(logging));

    let server = warp::post()
        .and(warp::path("proxy"))
        .and(warp::path::end())
        .and(json_body())
        .and_then(handle_request);

    let addr: SocketAddr = "0.0.0.0:3000".parse().unwrap();

    warp::serve(server).run(addr).await;

    Ok(())
}
