//! System numbers for the dashboard. Reading `/proc` costs a few file reads and
//! no extra memory; on systems without `/proc` the numbers stay at zero and the
//! dashboard says so.

use serde_json::{json, Value};
use std::path::Path;

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn mem_info() -> (u64, u64) {
    let text = read("/proc/meminfo");
    let mut total = 0u64;
    let mut available = 0u64;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or("");
        let value = parts
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        if key == "MemTotal:" {
            total = value;
        }
        if key == "MemAvailable:" {
            available = value;
        }
    }
    (total, available)
}

fn cpu_usage() -> f64 {
    let text = read("/proc/stat");
    let first = text.lines().next().unwrap_or("");
    let values: Vec<u64> = first
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    if values.len() < 4 {
        return 0.0;
    }
    let idle = values[3];
    let total: u64 = values.iter().sum();
    if total == 0 {
        return 0.0;
    }
    ((total - idle) as f64 / total as f64) * 100.0
}

fn uptime() -> u64 {
    read("/proc/uptime")
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v as u64)
        .unwrap_or(0)
}

fn cores() -> u64 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u64)
        .unwrap_or(1)
}

/// Memory this server itself is using, in kB (`/proc/self/statm`).
fn own_memory() -> u64 {
    let text = read("/proc/self/statm");
    let pages: u64 = text
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    pages * 4 // 4 kB pages
}

fn network_totals() -> (u64, u64) {
    let text = read("/proc/net/dev");
    let mut rx = 0u64;
    let mut tx = 0u64;
    for line in text.lines().skip(2) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 10 {
            continue;
        }
        rx += parts[1].parse::<u64>().unwrap_or(0);
        tx += parts[9].parse::<u64>().unwrap_or(0);
    }
    (rx, tx)
}

pub fn snapshot(db: &crate::store::Db) -> Value {
    let (mem_total, mem_available) = mem_info();
    let mem_used = mem_total.saturating_sub(mem_available);
    let (net_in, net_out) = network_totals();
    let nginx_path = crate::nginx::nginx_path(db);

    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpuCores": cores(),
        "cpuUsage": (cpu_usage() * 10.0).round() / 10.0,
        "uptime": uptime(),
        "memTotal": mem_total,
        "memUsed": mem_used,
        "memPercent": if mem_total > 0 { (mem_used as f64 / mem_total as f64 * 1000.0).round() / 10.0 } else { 0.0 },
        "processMem": own_memory(),
        "netIn": net_in,
        "netOut": net_out,
        "nginxRunning": crate::nginx::running(db),
        "nginxPath": nginx_path.display().to_string(),
        "home": db.setting("home"),
        "version": crate::VERSION,
        "javaFree": true,
        "startedAt": crate::started_at(),
        "logDir": crate::nginx::nginx_dir(db).join("logs").display().to_string(),
        "webDir": "",
        "hasProc": Path::new("/proc/meminfo").exists(),
    })
}
