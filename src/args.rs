use std::net::SocketAddr;
use structopt::clap::AppSettings;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt(global_settings = &[AppSettings::ColoredHelp])]
pub struct Args {
    /// Only show warnings
    #[structopt(short, long, global = true)]
    pub quiet: bool,
    /// More verbose logs
    #[structopt(short, long, global = true, parse(from_occurrences))]
    pub verbose: u8,
    /// The address to bind to
    #[structopt(short = "B", long, default_value = "127.0.0.1:1080")]
    pub bind: SocketAddr,
    //the backend url for live_proxies (for convenience a default value should be added)
    #[structopt(short = "U", long, default_value = "http://127.0.0.1:8000")]
    pub url: String,
    //the live_proxies file
    #[structopt(short = "L", default_value = "./proxies.txt")]
    pub proxies: String,
}
