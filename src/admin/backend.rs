use crate::{Protocol, ProxyData, ProxyInfo, Traffic};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg_attr(test, derive(Debug, Default, PartialEq))]
#[derive(Serialize, Deserialize)]
pub struct Proxy {
    protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub url: String,
    #[serde(rename = "isDeleted")]
    is_deleted: bool,
    id: String,
}

#[cfg_attr(test, derive(Debug, Default, PartialEq))]
#[derive(Serialize, Deserialize)]
pub struct ClientData {
    pub user: String,
    pub proxy: Proxy,
    protocol: Protocol,
    pub m_username: String,
    pub m_password: String,
    pub allow_ip: Vec<String>,
    pub m_host: String,
    pub m_port: u16,
    pub data_in: u64,
    pub data_out: u64,
    created: String,
    updated: String,
    pub m_url: String,
    #[serde(rename = "isDeleted")]
    is_deleted: bool,
    id: String,
}

#[cfg_attr(test, derive(Debug, Default, PartialEq))]
#[derive(Serialize, Deserialize)]
pub struct BackendData {
    pub data: Vec<ClientData>,
}

#[derive(Serialize, Deserialize)]
pub struct ProxyHashMap {
    pub hashmap: HashMap<u16, ProxyData>,
}

impl From<BackendData> for ProxyHashMap {
    fn from(backend_data: BackendData) -> Self {
        let mut hashmap = HashMap::new();
        for data in backend_data.data {
            if let Ok(proxy_addr) = format!("{}:{}", data.proxy.host, data.proxy.port).parse() {
                let proxy_info = ProxyInfo {
                    protocol: data.protocol,
                    proxy_addr,
                    username: data.proxy.username,
                    password: data.proxy.password,
                    m_credentials: (data.m_username, data.m_password),
                };
                let proxy_data = ProxyData {
                    info: proxy_info,
                    traffic: Traffic(data.data_out, data.data_in),
                };
                hashmap.insert(data.m_port, proxy_data);
            }
        }
        Self { hashmap }
    }
}

pub async fn get_proxies(url: String) -> Result<BackendData, reqwest::Error> {
    reqwest::get(url)
        .await?
        .json::<Vec<ClientData>>()
        .await
        .map(|data| BackendData { data })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use actix_web::{get, App, HttpResponse, HttpServer, Responder};
    use tokio::test;

    #[get("/")]
    async fn run() -> impl Responder {
        let mut client_data = ClientData::default();
        client_data.user = "test".to_string();
        let data = vec![client_data];
        let json_data = serde_json::to_string(&data).unwrap();
        HttpResponse::Ok()
            .content_type("application/json")
            .body(json_data)
    }

    async fn backend() -> std::io::Result<()> {
        HttpServer::new(|| App::new().service(run))
            .bind("localhost:8000")
            .unwrap()
            .run()
            .await
    }

    #[test]
    async fn test_backend() {
        let timeout = std::time::Duration::from_secs(1);
        //the backend should run forever so Err is our Ok case
        if let Err(_) = tokio::time::timeout(timeout, backend()).await {
            assert!(true);
        } else {
            assert!(false);
        }
    }

    #[test]
    pub async fn test_get_proxies() {
        let url = "http://localhost:8000".to_string();
        let mut client_data = ClientData::default();
        client_data.user = "test".to_string();
        let result = get_proxies(url).await;
        if let Ok(res) = result {
            if !res.data.is_empty() {
                println!("client_data: {:?}", res);
                assert_eq!(res.data[0], client_data);
            } else {
                assert!(false)
            }
        } else {
            assert!(false);
        }
    }
}
