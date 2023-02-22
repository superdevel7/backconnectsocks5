// https://github.com/rust-lang/rust-clippy/issues/7271
#![allow(clippy::needless_lifetimes)]

pub mod args;
pub mod errors;
pub mod list;
pub mod socks5;

use crate::args::Args;
use crate::errors::*;
use anyhow::Result;
// use arc_swap::ArcSwap;
/// Importing the `DateTime` type from the `chrono` crate.
// use chrono::prelude::*;
use env_logger::Env;
use lazy_static::lazy_static;
/// Importing all the traits that are needed to use the mysql crate.
use mysql::prelude::*;
/// Importing all the traits that are needed to use the mysql crate.
use mysql::*;
use parking_lot::Mutex;
use rand::seq::SliceRandom;
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::net::SocketAddr;
// use std::path::PathBuf;
// use std::sync::Arc;
use structopt::StructOpt;
use tokio::net::TcpListener;
// use tokio::signal::unix::{signal, SignalKind};
use rand::Rng;
use warp::http::StatusCode;
use warp::Filter;

#[derive(Deserialize, Serialize)]
pub struct Body {
    pub country: String,
    pub period: i32,
}

#[derive(Deserialize, Serialize)]
pub struct ResponseData {
    pub port: u16,
}

#[derive(Deserialize, Serialize)]
pub struct ErrMessage {
    pub message: String,
}

lazy_static! {
    static ref CONN: Mutex<PooledConn> = {
        let url = "mysql://root:Test!234@127.0.0.1:3306/backconnect";
        let pool = Pool::new(url).unwrap();
        let m = pool.get_conn().unwrap();
        Mutex::new(m)
    };
}

pub async fn run_server(address: String, port: String, p: u16) -> Result<()> {
    let addr: String = format!("{}:{}", address, port).parse()?;

    let proxy: SocketAddr = addr.parse().unwrap();

    let bind: SocketAddr = format!("0.0.0.0:{}", p).parse().unwrap();
    // a stream of sighup signals
    // let mut sighup = signal(SignalKind::hangup())?;

    // let proxies = list::load_from_path(&proxy_list)
    //     .await
    //     .context("Failed to load proxy list")?;
    // let proxies = ArcSwap::from(Arc::new(proxies));

    info!("Binding listener to {}", bind);
    let listener = TcpListener::bind(bind).await?;

    tokio::spawn(async move {
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
                    tokio::spawn(async move {
                        if let Err(err) = socks5::serve_one(socket, proxy.clone()).await {
                            warn!("Error serving client: {:#}", err);
                        }
                    });
                }
                // _ = sighup.recv() => {
                //     debug!("Got signal HUP");
                //     match list::load_from_path(&proxy_list).await {
                //         Ok(list) => {
                //             let list = Arc::new(list);
                //             proxies.store(list);
                //         }
                //         Err(err) => {
                //             error!("Failed to reload proxy list: {:#}", err);
                //         }
                //     }
                // }
            }
        }
    });

    Ok(())
}

pub async fn handle_request(body: Body) -> Result<impl warp::Reply, Infallible> {
    debug!(
        "Got new client connection for country = {}, period = {}",
        body.country, body.period
    );
    let list: Vec<(String, String)> = CONN
        .lock()
        .query(format!(
            "select address, port from proxies where country='{}' and status=1 and period='{}'",
            body.country, body.period
        ))
        .unwrap();

    if list.is_empty() {
        // error!("Couldn't find proxy server for {} {} minutes", body.country, body.period);
        let data = ErrMessage {
            message: "No proxy server found".to_string(),
        };
        Ok(warp::reply::with_status(
            warp::reply::json(&data),
            StatusCode::BAD_REQUEST,
        ))
    } else {
        let one = list.choose(&mut thread_rng()).unwrap();
        let p: u16;

        {
            let mut rng = rand::thread_rng();
            p = rng.gen_range(30000..60000);
        }

        // // tokio::spawn(async move {
        // //     // if let Err(err) = run_server(one.0.clone(), one.1.clone()).await {
        // //     //     warn!("Error running server: {:#}", err);
        // //     // }
        // //     run_server(one.0.clone(), one.1.clone());
        // // });

        // // data.port = p;
        match run_server(one.0.clone(), one.1.clone(), p).await {
            Ok(_) => {
                //let success_200 = warp::any().map(warp::reply::json(&data));
                // let reply = warp::reply::json(&data);
                // let reply = Box::new(warp::reply::json(&data))
                //     .map(|reply| warp::reply::with_status(reply, StatusCode::OK));
                // Ok(warp::reply::with_status(reply, StatusCode::OK))
                let data = ResponseData { port: p };
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
}

fn json_body() -> impl Filter<Extract = (Body,), Error = warp::Rejection> + Clone {
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}

#[tokio::main]
pub async fn main() -> Result<()> {
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
        .and(warp::path!("api" / "proxy" / "buy"))
        .and(warp::path::end())
        .and(json_body())
        .and_then(handle_request);

    warp::serve(server).run(args.bind).await;

    Ok(())
}
