//! Everything that touches the file system and the nginx program: where the
//! files live, writing the generated configuration, keeping a backup of the last
//! one, and starting / testing / reloading nginx.

use crate::conf;
use crate::store::{now_ms, str_of, Db, Store};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn unix_is_file(path: &str) -> bool {
    cfg!(unix) && Path::new(path).is_file()
}

pub fn unix_is_dir(path: &str) -> bool {
    cfg!(unix) && Path::new(path).is_dir()
}

pub fn nginx_path(db: &Db) -> PathBuf {
    let setting = db.setting("nginxPath");
    if setting.trim().is_empty() {
        PathBuf::from(db.setting("home")).join("nginx.conf")
    } else {
        PathBuf::from(setting)
    }
}

pub fn nginx_dir(db: &Db) -> PathBuf {
    let setting = db.setting("nginxDir");
    if !setting.trim().is_empty() {
        return PathBuf::from(setting);
    }
    nginx_path(db)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn which_nginx() -> Option<PathBuf> {
    crate::install::which("nginx").or_else(|| crate::install::which("/usr/sbin/nginx"))
}

/// Read `--conf-path` / `--prefix` / `--sbin-path` from `nginx -V`.
pub fn probe_binary(exe: &str) -> Option<(PathBuf, PathBuf, PathBuf)> {
    if exe.trim().is_empty() {
        return None;
    }
    let output = Command::new(exe).arg("-V").output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    parse_nginx_v(&text)
}

pub fn parse_nginx_v(text: &str) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let arg = |key: &str| -> Option<PathBuf> {
        let pat = format!("--{key}=");
        text.split_whitespace()
            .find_map(|p| p.strip_prefix(&pat))
            .map(PathBuf::from)
    };
    let conf = arg("conf-path")?;
    let prefix = arg("prefix").unwrap_or_else(|| {
        conf.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    });
    let sbin = arg("sbin-path").unwrap_or_else(|| PathBuf::from("nginx"));
    Some((conf, prefix, sbin))
}

pub fn nginx_exe(db: &Db) -> String {
    let exe = db.setting("nginxExe");
    if exe.trim().is_empty() {
        "nginx".to_string()
    } else {
        exe.trim().to_string()
    }
}

pub fn decompose(db: &Db) -> bool {
    db.setting("decompose") == "true"
}

/// The generated text plus the small files it would write, without writing.
pub fn preview(db: &Db) -> conf::ConfExt {
    conf::build_conf(db, decompose(db))
}

const MANAGED_PREFIXES: [&str; 6] = ["http_", "tcp_", "udp_", "upstream_", "rewrite_", "deny_"];

fn is_managed(name: &str) -> bool {
    name.ends_with(".conf") && MANAGED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Write the configuration, keeping the previous one as a backup.
pub fn write_conf(store: &mut Store) -> Result<Vec<String>, String> {
    let (conf_text, files, split, main_path, dir) = {
        let db = &store.db;
        let conf_ext = conf::build_conf(db, decompose(db));
        (
            conf_ext.conf,
            conf_ext.files,
            decompose(db),
            nginx_path(db),
            nginx_dir(db),
        )
    };
    let conf_d = dir.join("conf.d");

    let mut written: Vec<String> = Vec::new();

    // backup the current file before overwriting it
    let previous = fs::read_to_string(&main_path).unwrap_or_default();
    if !previous.trim().is_empty() && !previous.contains(conf::GENERATED_MARK) {
        return Err(
            "refuseOverwriteNginxConf".into(),
        );
    }
    if !previous.trim().is_empty() {
        let time = now_ms().to_string();
        let mut bak = serde_json::Map::new();
        bak.insert("id".into(), time.clone().into());
        bak.insert("time".into(), time.clone().into());
        bak.insert("content".into(), previous.into());
        store.db.upsert("bak", serde_json::Value::Object(bak));
        // keep the last 20 backups only, so the data file stays small
        let mut ids: Vec<String> = store
            .db
            .rows("bak")
            .iter()
            .map(|r| str_of(r, "id"))
            .collect();
        ids.sort();
        if ids.len() > 20 {
            for id in &ids[..ids.len() - 20] {
                store.db.delete("bak", id);
            }
        }
    }

    if let Some(parent) = main_path.parent() {
        crate::privfs::mkdir_all(parent)?;
    }
    crate::privfs::write(&main_path, &conf_text)?;
    written.push(main_path.display().to_string());

    crate::privfs::mkdir_all(&conf_d)?;
    let mut wanted: Vec<String> = Vec::new();
    if split {
        for file in &files {
            crate::privfs::write(&conf_d.join(&file.name), &file.conf)?;
            wanted.push(file.name.clone());
            written.push(conf_d.join(&file.name).display().to_string());
        }
    }
    // remove files we created earlier that are no longer part of the config
    if let Ok(entries) = fs::read_dir(&conf_d) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_managed(&name) && !wanted.contains(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    Ok(written)
}

fn run(program: &str, args: &[String], cwd: &Path) -> (bool, String) {
    let mut command = Command::new(program);
    command.args(args);
    if cwd.exists() {
        command.current_dir(cwd);
    }
    command.stdin(Stdio::null());
    match command.output() {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            (output.status.success(), text)
        }
        Err(err) => (false, format!("{}: {}", program, err)),
    }
}

fn nginx_args(db: &Db, extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
    let conf_path = nginx_path(db);
    if !db.setting("nginxPath").trim().is_empty() {
        args.push("-c".into());
        args.push(conf_path.display().to_string());
    }
    // Debian nginx uses its own prefix. Only pass -p when the operator set one.
    let prefix = db.setting("nginxPrefix");
    if !prefix.trim().is_empty() {
        args.push("-p".into());
        args.push(format!("{}/", prefix.trim()));
    }
    args
}

pub fn test_conf(db: &Db) -> (bool, String) {
    run(&nginx_exe(db), &nginx_args(db, &["-t"]), &nginx_dir(db))
}

pub fn reload(db: &Db) -> (bool, String) {
    let cmd = db.setting("reloadCmd");
    if !cmd.trim().is_empty() {
        return run_command(db, cmd.trim());
    }
    run(
        &nginx_exe(db),
        &nginx_args(db, &["-s", "reload"]),
        &nginx_dir(db),
    )
}

pub fn signal(db: &Db, action: &str) -> (bool, String) {
    match action {
        "start" => run(&nginx_exe(db), &nginx_args(db, &[]), &nginx_dir(db)),
        "stop" => run(&nginx_exe(db), &nginx_args(db, &["-s", "stop"]), &nginx_dir(db)),
        "quit" => run(&nginx_exe(db), &nginx_args(db, &["-s", "quit"]), &nginx_dir(db)),
        "restart" => {
            let (ok, out) = run(&nginx_exe(db), &nginx_args(db, &["-s", "stop"]), &nginx_dir(db));
            let (ok2, out2) = run(&nginx_exe(db), &nginx_args(db, &[]), &nginx_dir(db));
            (ok || ok2, format!("{}\n{}", out, out2))
        }
        _ => (false, "unknown action".to_string()),
    }
}

/// Is nginx running? Debian uses /run/nginx.pid; a prefix install uses logs/nginx.pid.
pub fn running(db: &Db) -> bool {
    let dir = nginx_dir(db);
    let candidates = [
        dir.join("logs").join("nginx.pid"),
        PathBuf::from("/run/nginx.pid"),
        PathBuf::from("/var/run/nginx.pid"),
        PathBuf::from("/usr/local/nginx/logs/nginx.pid"),
    ];
    for pid_file in &candidates {
        if let Ok(text) = fs::read_to_string(pid_file) {
            let pid = text.trim();
            if pid.is_empty() {
                continue;
            }
            if Path::new("/proc").is_dir() {
                if Path::new(&format!("/proc/{}", pid)).exists() {
                    return true;
                }
            } else {
                return true;
            }
        }
    }
    proc_named_nginx()
}

/// Directories that may hold access/error logs. Debian uses /var/log/nginx.
pub fn log_dirs(db: &Db) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let debian = PathBuf::from("/var/log/nginx");
    if debian.is_dir() {
        dirs.push(debian);
    }
    let local = nginx_dir(db).join("logs");
    if !dirs.iter().any(|d| d == &local) {
        dirs.push(local);
    }
    dirs
}

pub fn find_log(db: &Db, name: &str) -> PathBuf {
    for dir in log_dirs(db) {
        let path = dir.join(name);
        if path.is_file() {
            return path;
        }
    }
    log_dirs(db)
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(name)
}

fn proc_named_nginx() -> bool {
    let entries = match fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let comm = fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        if comm.trim() == "nginx" {
            return true;
        }
    }
    false
}

/// Only commands without shell tricks are allowed, like the Java version's check.
pub fn command_is_safe(command: &str) -> bool {
    !(command.contains("&&")
        || command.contains("||")
        || command.contains(';')
        || command.contains('`')
        || command.contains("$(")
        || command.contains('|')
        || command.contains('>')
        || command.contains('<')
        || command.contains('\n'))
}

pub fn run_command(db: &Db, command: &str) -> (bool, String) {
    let parts: Vec<String> = command
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if parts.is_empty() {
        return (false, "empty command".to_string());
    }
    let mut argv = parts.clone();
    let program = argv.remove(0);
    let mut full = argv;
    let conf_path = nginx_path(db);
    for arg in full.iter_mut() {
        if arg.contains("nginx.conf") {
            *arg = arg.replace("nginx.conf", &conf_path.display().to_string());
        }
    }
    run(&program, &full, &nginx_dir(db))
}

/// Tail of a log file: read only the last part, so a huge log cannot eat memory.
pub fn tail(path: &Path, max_bytes: u64, max_lines: usize) -> String {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) => return format!("cannot read {}: {}", path.display(), err),
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = size.saturating_sub(max_bytes);
    let _ = file.seek(SeekFrom::Start(start));
    let mut buffer = Vec::new();
    let _ = file.read_to_end(&mut buffer);
    let text = String::from_utf8_lossy(&buffer);
    let lines: Vec<&str> = text.lines().collect();
    let from = lines.len().saturating_sub(max_lines);
    lines[from..].join("\n")
}

// ------------------------------------------------------------ password files

fn to64(mut value: u32, count: usize, out: &mut String) {
    const ALPHABET: &[u8] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    for _ in 0..count {
        out.push(ALPHABET[(value & 0x3f) as usize] as char);
        value >>= 6;
    }
}

/// Apache-style MD5 password hash (`$apr1$...`), which nginx accepts in an
/// `auth_basic_user_file` on Linux. Windows keeps the plain text the Java
/// version wrote there.
pub fn apr1(pass: &str, salt: &str) -> String {
    md5_crypt(pass, salt, "$apr1$")
}

/// The shared MD5-crypt algorithm; `magic` is `$1$` for the classic Unix
/// variant and `$apr1$` for Apache's.
pub fn md5_crypt(pass: &str, salt: &str, magic: &str) -> String {
    use md5::{Digest, Md5};
    let salt: String = salt.chars().take(8).collect();
    let pass_bytes = pass.as_bytes();
    let salt_bytes = salt.as_bytes();

    let mut ctx = Md5::new();
    ctx.update(pass_bytes);
    ctx.update(magic.as_bytes());
    ctx.update(salt_bytes);

    let mut inner = Md5::new();
    inner.update(pass_bytes);
    inner.update(salt_bytes);
    inner.update(pass_bytes);
    let inner_sum = inner.finalize();

    let mut left = pass_bytes.len();
    while left > 0 {
        let take = left.min(16);
        ctx.update(&inner_sum[..take]);
        left -= take;
    }

    let mut bits = pass_bytes.len();
    while bits != 0 {
        if bits & 1 == 1 {
            ctx.update([0u8]);
        } else {
            ctx.update(&pass_bytes[..1.min(pass_bytes.len())]);
        }
        bits >>= 1;
    }

    let mut sum = ctx.finalize().to_vec();

    for round in 0..1000 {
        let mut ctx1 = Md5::new();
        if round & 1 == 1 {
            ctx1.update(pass_bytes);
        } else {
            ctx1.update(&sum);
        }
        if round % 3 != 0 {
            ctx1.update(salt_bytes);
        }
        if round % 7 != 0 {
            ctx1.update(pass_bytes);
        }
        if round & 1 == 1 {
            ctx1.update(&sum);
        } else {
            ctx1.update(pass_bytes);
        }
        sum = ctx1.finalize().to_vec();
    }

    let mut out = String::new();
    out.push_str(magic);
    out.push_str(&salt);
    out.push('$');

    let c = |i: usize| sum[i] as u32;
    let mut l = (c(0) << 16) | (c(6) << 8) | c(12);
    to64(l, 4, &mut out);
    l = (c(1) << 16) | (c(7) << 8) | c(13);
    to64(l, 4, &mut out);
    l = (c(2) << 16) | (c(8) << 8) | c(14);
    to64(l, 4, &mut out);
    l = (c(3) << 16) | (c(9) << 8) | c(15);
    to64(l, 4, &mut out);
    l = (c(4) << 16) | (c(10) << 8) | c(5);
    to64(l, 4, &mut out);
    to64(c(11), 2, &mut out);

    out
}

pub fn write_password_file(_store: &Store, path: &Path, user: &str, pass: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let value = if cfg!(windows) {
        format!("{}:{}", user, pass)
    } else {
        let salt = crate::auth::random_token(8);
        format!("{}:{}", user, apr1(pass, &salt))
    };
    fs::write(path, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_crypt_matches_php_docs() {
        // published vector: crypt('rasmuslerdorf', '$1$rasmusle$')
        assert_eq!(
            md5_crypt("rasmuslerdorf", "rasmusle", "$1$"),
            "$1$rasmusle$rISCgZzpwk3UhDidwXvin0"
        );
    }

    #[test]
    fn parses_nginx_v_conf_path() {
        let text = "configure arguments: --prefix=/usr/share/nginx --sbin-path=/usr/sbin/nginx --conf-path=/etc/nginx/nginx.conf --pid-path=/run/nginx.pid";
        let (conf, prefix, sbin) = parse_nginx_v(text).unwrap();
        assert_eq!(conf, PathBuf::from("/etc/nginx/nginx.conf"));
        assert_eq!(prefix, PathBuf::from("/usr/share/nginx"));
        assert_eq!(sbin, PathBuf::from("/usr/sbin/nginx"));
    }

    #[test]
    fn generated_mark_blocks_foreign_conf() {
        assert!(crate::conf::GENERATED_MARK.starts_with('#'));
        let foreign = "user www-data;\nhttp { include sites-enabled/*; }\n";
        assert!(!foreign.contains(crate::conf::GENERATED_MARK));
        let ours = format!("{}\nworker_processes auto;\n", crate::conf::GENERATED_MARK);
        assert!(ours.contains(crate::conf::GENERATED_MARK));
    }

    #[test]
    fn tails_only_the_end() {
        let dir = std::env::temp_dir().join("nginxwebui-test-tail");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("a.log");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        assert_eq!(tail(&file, 6, 10), "three");
    }

    #[test]
    fn command_is_safe_rejects_shell_tricks() {
        assert!(command_is_safe("sudo systemctl reload nginx"));
        assert!(!command_is_safe("sudo systemctl reload nginx || true"));
        assert!(!command_is_safe("rm -rf / | nginx"));
    }
}
