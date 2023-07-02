use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use tokio::{sync::{ RwLock, mpsc::{error::TryRecvError, UnboundedReceiver, UnboundedSender}},task::{self,JoinHandle}};
use crate::{ThreadMessage, ProxyHandle, Traffic, AdminProxyBody, ProxyData, admin::backend::get_proxies};
use log::{debug,info,warn,error};
use anyhow::{anyhow, ensure, Result};

mod backend;
use backend::ProxyHashMap;

#[derive(Deserialize, Serialize)]
struct AdminData {
    m_port: u16,
    delay: u64,
    bytes_sent: u64,
    bytes_received: u64,
    active_threads: usize
}

async fn run_admin_endpoint( body: AdminProxyBody, proxies: Arc<RwLock<HashMap<u16, ProxyHandle>>>, thread_counter: Arc<AtomicUsize>) 
                    -> Result<AdminData> {
     let m_port = body.m_port;                   
     let mut proxies_hashmap = proxies.write().await;
     let handle = proxies_hashmap.get_mut(&m_port).ok_or(anyhow!("Port {} is not in use", m_port))?;
     handle.delay.store(body.delay, Ordering::Release);
     let (delay, Traffic(bytes_sent, bytes_received)) = (handle.delay.load(Ordering::Acquire), handle.traffic);
     ensure!(delay == body.delay, "Error on updating delay for m_port: {}.", body.m_port);
     let active_threads = thread_counter.load(Ordering::Acquire);
     Ok(AdminData{m_port, delay, bytes_sent, bytes_received, active_threads})
}

pub async fn handle_admin_endpoint(
    (body, proxies, thread_counter): 
    (AdminProxyBody, Arc<RwLock<HashMap<u16, ProxyHandle>>>, Arc<AtomicUsize>)
) -> Result<impl warp::Reply, std::convert::Infallible> {

    match run_admin_endpoint(body, proxies, thread_counter).await
    {
        Ok(data) => {
            Ok(warp::reply::with_status(
                warp::reply::json(&data),
                warp::http::StatusCode::OK,
            ))   
        }
        Err(err) => {
            // fail
            error!("Failed to run admin endpoint: {:#}", err);
            let data =crate::ErrMessage {
                message: err.to_string()
            };
            Ok(warp::reply::with_status(
                warp::reply::json(&data),
                warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

//load the live_proxies hashmap from file
// async fn live_proxies_load(path: &str) -> Result<HashMap<u16, ProxyData>> {
//     use tokio::io::AsyncReadExt;
//     let mut file = tokio::fs::File::open(path).await?;
//     let mut contents = String::new();
//     file.read_to_string(&mut contents).await?;
//     Ok(serde_json::from_str(&contents)?)
//}
//save the live_proxies hashmap to file
async fn live_proxies_save (proxies: &HashMap<u16, ProxyData>, path: &str) -> tokio::io::Result<()>{
    use tokio::io::AsyncWriteExt;
    let json = serde_json::to_string(proxies)?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(path)
        .await?;
    file.write_all(json.as_bytes()).await
}
// get the live_proxies hashmap from file on start
// async fn proxies_restore(proxies: Arc<RwLock<HashMap<u16, ProxyHandle>>>, 
//                          tx: UnboundedSender<ThreadMessage>, path: &str) -> HashMap<u16, ProxyData> {
//     if let Ok(live_proxies) = live_proxies_load(path).await {
//         for (m_port, proxy_data) in &live_proxies {
//             let proxy_data = ProxyData { info: proxy_data.info.clone(), traffic: proxy_data.traffic.clone()};
//             if let Err(err) = crate::run_server(*m_port, proxy_data, Vec::new(), proxies.clone(), tx.clone()).await {
//                 error!("Cannot restart proxy on {m_port}. Error: {err}");
//             }
//         } 
//         live_proxies
//     } else { HashMap::new() }
// }

//starts all the proxies from live_proxies hashmap
async fn live_proxies_init(live_proxies: &HashMap<u16, ProxyData>, proxies: Arc<RwLock<HashMap<u16, ProxyHandle>>>, 
                          tx: UnboundedSender<ThreadMessage>) {
    for (m_port, proxy_data) in live_proxies {
        let proxy_data = ProxyData { info: proxy_data.info.clone(), traffic: proxy_data.traffic.clone()};
        if let Err(err) = crate::run_server(*m_port, proxy_data, Vec::new(), proxies.clone(), tx.clone()).await {
            error!("Cannot restart proxy on {m_port}. Error: {err}");
        }
    }
}

//this thread will stay alive as long as the server is running and monitor the proxies traffic
pub async fn start_admin_thread(mut rx: UnboundedReceiver<ThreadMessage>, tx: UnboundedSender<ThreadMessage>, proxies_path: String,
                                proxies: Arc<RwLock<HashMap<u16, ProxyHandle>>>, thread_counter: Arc<AtomicUsize>, url: String) -> Result<()>{                             
    let admin =task::spawn( async move {                
        debug!("Admin thread started");
        let mut messages: Vec<(u16, JoinHandle<Option<(u64,u64)>>)> = Vec::new();
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(1));
        //let mut live_proxies: HashMap<u16, ProxyData> = proxies_restore(proxies.clone(), tx, &proxies_path).await;
        // the live_proxies will load now from backend_server
        let mut live_proxies: HashMap<u16, ProxyData> = 
                match get_proxies(url.clone()).await {
                    Ok(backend_data) => Into::<ProxyHashMap>::into(backend_data).hashmap,
                    Err(err) => {
                        error!("Error on getting proxies list from backend: {err}");
                        HashMap::new()
                    }
        };
        live_proxies_init(&live_proxies, proxies.clone(), tx).await;
        let mut start = tokio::time::Instant::now();
        let one_day = std::time::Duration::from_secs(3600*24); 
        loop {
            match rx.try_recv() {
                Ok(ThreadMessage::StartProxy(m_port, delay, proxy_info)) => {
                    let mut proxies_traffic = proxies.write().await;
                    if let Some(proxy_handle) = proxies_traffic.get_mut(&m_port) {
                        proxy_handle.delay = delay;
                    }
                    live_proxies.insert(m_port, ProxyData{ info: proxy_info, traffic: Traffic::default()});
                    if let Err(err) = live_proxies_save(&live_proxies, &proxies_path).await {
                        error!("Failed to save the live_proxies for port {m_port}. Error: {err}");
                    }
                }
                Ok(ThreadMessage::StopProxy(m_port)) => { 
                    live_proxies.remove(&m_port);
                    if let Err(err) = live_proxies_save(&live_proxies, &proxies_path).await {
                        error!("Failed to remove proxy {m_port} from live_proxies. Error: {err}");
                    }
                }
                Ok(ThreadMessage::NewServeOne(m_port, join_handle)) => messages.push((m_port, join_handle)),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break
            }
            heartbeat.tick().await;
            let finnished: Vec<(u16, JoinHandle<Option<(u64,u64)>>)>;
            (messages, finnished) = messages.into_iter().partition(|m| !m.1.is_finished());
            for message in finnished {
                match message.1.await {
                    Ok(bytes) => {
                        let mut proxies_traffic = proxies.write().await;
                        let (bytes_sent, bytes_received) = bytes.unwrap_or((0,0));
                        if let Some(proxy_handle) = proxies_traffic.get_mut(&message.0) {
                            proxy_handle.traffic += Traffic(bytes_sent, bytes_received);
                            live_proxies.entry(message.0).and_modify(|proxy| proxy.traffic = proxy_handle.traffic);
                        }
                    }
                    Err(err) => warn!("Finished thread with error: {:?}", err)
                }    
            }
            // update the proxies file daily
            let now = tokio::time::Instant::now();
            if now > start + one_day {
                start = now; 
                if let Err(err) = live_proxies_save(&live_proxies, &proxies_path).await {
                    error!("Failed the daily update for live_proxies. Error: {err}"); 
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
                info!("Admin thread: number of active threads: {:?}", thread_counter);
            }
        }
    });
    ensure!(!admin.is_finished(), "Error on starting the admin thread");
    Ok(())
}