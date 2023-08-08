use actix_web::{get, App, HttpResponse, HttpServer, Responder};
use serde::{Deserialize, Serialize};
use tokio::test;

#[cfg_attr(test, derive(Debug, Default, PartialEq))]
#[derive(Serialize, Deserialize)]
enum Protocol {
    #[cfg_attr(test, default)]
    Socks5,
    Http,
}

impl std::string::ToString for Protocol {
    fn to_string(&self) -> String {
        match self {
            Protocol::Socks5 => String::from("socks5"),
            Protocol::Http => String::from("http"),
        }
    }
}

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
    pub data_in: u16,
    pub data_out: u16,
    created: String,
    updated: String,
    pub m_url: String,
    #[serde(rename = "isDeleted")]
    is_deleted: bool,
    id: String,
}

#[get("/")]
async fn run() -> impl Responder {
    let mut client_data = ClientData::default();
    client_data.user = "test".to_string();
    client_data.allow_ip = vec!["127.0.0.1".to_string()];
    client_data.m_port = 9999;
    client_data.proxy.port = 47049;
    client_data.proxy.host = "87.106.127.114".to_string();
    let backend_data = vec![client_data];
    println!("backend_data: {:?}", backend_data);
    let json_data = serde_json::to_string(&backend_data).unwrap();
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
    let timeout = std::time::Duration::from_secs(10);
    //the backend should run forever so Err is our Ok case
    if let Err(_) = tokio::time::timeout(timeout, backend()).await {
        assert!(true);
    } else {
        assert!(false);
    }
}
