//! Shared library for the nginxWebUI panel and the outbound `nginxwebui-agent`.

pub mod acme;
pub mod agent;
pub mod api;
pub mod applog;
pub mod auth;
pub mod compat;
pub mod conf;
pub mod config;
pub mod install;
pub mod net;
pub mod nginx;
pub mod privfs;
pub mod registry;
pub mod rpc;
pub mod secrets;
pub mod seed;
mod server;
pub mod site;
pub mod store;
pub mod sysinfo;
pub mod www;

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

static STARTED_AT: OnceLock<u64> = OnceLock::new();

pub fn started_at() -> u64 {
    *STARTED_AT.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

pub fn run() {
    server::run();
}
