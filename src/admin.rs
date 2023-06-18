use crate::{AdminProxyBody, ProxyHandle, ThreadMessage, Traffic};
use anyhow::{anyhow, ensure, Result};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    sync::{
        mpsc::{error::TryRecvError, UnboundedReceiver},
        RwLock,
    },
    task::{self, JoinHandle},
};

#[derive(serde::Deserialize, serde::Serialize)]
struct AdminData {
    m_port: u16,
    delay: u64,
    bytes_sent: u64,
    bytes_received: u64,
    active_threads: usize,
}

async fn run_admin_endpoint(
    body: AdminProxyBody,
    proxies: Arc<RwLock<HashMap<u16, ProxyHandle>>>,
    thread_counter: Arc<AtomicUsize>,
) -> Result<AdminData> {
    let m_port = body.m_port;
    let mut proxies_hashmap = proxies.write().await;
    let handle = proxies_hashmap
        .get_mut(&m_port)
        .ok_or(anyhow!("Port {} is not in use", m_port))?;
    handle.delay.store(body.delay, Ordering::Release);
    let (delay, Traffic(bytes_sent, bytes_received)) =
        (handle.delay.load(Ordering::Acquire), handle.traffic);
    ensure!(
        delay == body.delay,
        "Error on updating delay for m_port: {}.",
        body.m_port
    );
    let active_threads = thread_counter.load(Ordering::Acquire);
    Ok(AdminData {
        m_port,
        delay,
        bytes_sent,
        bytes_received,
        active_threads,
    })
}

pub async fn handle_admin_endpoint(
    (body, proxies, thread_counter): (
        AdminProxyBody,
        Arc<RwLock<HashMap<u16, ProxyHandle>>>,
        Arc<AtomicUsize>,
    ),
) -> Result<impl warp::Reply, std::convert::Infallible> {
    match run_admin_endpoint(body, proxies, thread_counter).await {
        Ok(data) => Ok(warp::reply::with_status(
            warp::reply::json(&data),
            warp::http::StatusCode::OK,
        )),
        Err(err) => {
            // fail
            error!("Failed to run admin endpoint: {:#}", err);
            let data = crate::ErrMessage {
                message: err.to_string(),
            };
            Ok(warp::reply::with_status(
                warp::reply::json(&data),
                warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

//this thread will stay alive as long as the server is running and monitor the proxies traffic
pub async fn start_admin_thread(
    mut rx: UnboundedReceiver<ThreadMessage>,
    proxies: Arc<RwLock<HashMap<u16, ProxyHandle>>>,
    thread_counter: Arc<AtomicUsize>,
) -> Result<()> {
    let admin = task::spawn(async move {
        debug!("Admin thread started");
        let mut messages: Vec<(u16, JoinHandle<Option<(u64, u64)>>)> = Vec::new();
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            match rx.try_recv() {
                Ok(ThreadMessage::StartProxy(m_port, delay)) => {
                    let mut proxies_traffic = proxies.write().await;
                    if let Some(proxy_handle) = proxies_traffic.get_mut(&m_port) {
                        proxy_handle.delay = delay;
                    }
                }
                Ok(ThreadMessage::NewServeOne(m_port, join_handle)) => {
                    messages.push((m_port, join_handle))
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }
            heartbeat.tick().await;
            let finnished: Vec<(u16, JoinHandle<Option<(u64, u64)>>)>;
            (messages, finnished) = messages.into_iter().partition(|m| !m.1.is_finished());
            for message in finnished {
                match message.1.await {
                    Ok(bytes) => {
                        let mut proxies_traffic = proxies.write().await;
                        let (bytes_sent, bytes_received) = bytes.unwrap_or((0, 0));
                        if let Some(proxy_handle) = proxies_traffic.get_mut(&message.0) {
                            proxy_handle.traffic += Traffic(bytes_sent, bytes_received);
                        }
                    }
                    Err(err) => warn!("Finished thread with error: {:?}", err),
                }
            }
            let count = thread_counter.load(Ordering::SeqCst);
            if count != messages.len() {
                thread_counter.store(messages.len(), Ordering::Release);
                for (key, _) in messages.iter() {
                    if let Some(proxy) = proxies.read().await.get(key) {
                        debug!("Proxy: {} - traffic yet recorded: (bytes_sent: {}, bytes_received: {}) - delay: {}", 
                                key, proxy.traffic.0, proxy.traffic.1, proxy.delay.load(Ordering::Acquire));
                    }
                }
                info!(
                    "Admin thread: number of active threads: {:?}",
                    thread_counter
                );
            }
        }
    });
    ensure!(!admin.is_finished(), "Error on starting the admin thread");
    Ok(())
}
