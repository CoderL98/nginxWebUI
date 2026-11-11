//! Small file + stderr logger. No extra crates: timestamps, levels, and a
//! single log file under `{home}/logs/nginxwebui.log`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

const ERROR: u8 = 1;
const WARN: u8 = 2;
const INFO: u8 = 3;
const DEBUG: u8 = 4;

static LEVEL: AtomicU8 = AtomicU8::new(INFO);
static FILE: Mutex<Option<File>> = Mutex::new(None);
static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn init(home: &Path, level: Option<&str>) {
    let wanted = level
        .map(str::to_string)
        .or_else(|| std::env::var("NGINXWEBUI_LOG_LEVEL").ok())
        .unwrap_or_else(|| "info".into());
    LEVEL.store(parse_level(&wanted), Ordering::Relaxed);
    let dir = home.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("nginxwebui.log");
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        *FILE.lock().unwrap() = Some(file);
        *PATH.lock().unwrap() = Some(path);
    }
}

fn parse_level(value: &str) -> u8 {
    match value.trim().to_ascii_lowercase().as_str() {
        "error" => ERROR,
        "warn" | "warning" => WARN,
        "debug" | "trace" => DEBUG,
        _ => INFO,
    }
}

fn write(level: u8, name: &str, message: &str) {
    if LEVEL.load(Ordering::Relaxed) < level {
        return;
    }
    let ts = crate::store::now_ms();
    let line = format!("{ts} {name} {message}");
    eprintln!("{line}");
    if let Ok(mut guard) = FILE.lock() {
        if let Some(file) = guard.as_mut() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
}

pub fn error(message: impl AsRef<str>) {
    write(ERROR, "ERROR", message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    write(WARN, "WARN", message.as_ref());
}

pub fn info(message: impl AsRef<str>) {
    write(INFO, "INFO", message.as_ref());
}

pub fn debug(message: impl AsRef<str>) {
    write(DEBUG, "DEBUG", message.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_levels() {
        assert_eq!(parse_level("error"), ERROR);
        assert_eq!(parse_level("WARN"), WARN);
        assert_eq!(parse_level("info"), INFO);
        assert_eq!(parse_level("debug"), DEBUG);
    }
}
