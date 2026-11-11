//! Writes that may need `sudo -n` when the process is not root.
//! Direct I/O first; on EACCES retry only the root-owned helper at
//! `/usr/local/sbin/nginxwebui-priv`. No unrestricted `sudo mkdir`/`install`.

use std::fs;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn denied(err: &std::io::Error) -> bool {
    err.kind() == ErrorKind::PermissionDenied
}

fn user_name() -> String {
    std::env::var("SUDO_USER")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "USER".into())
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

fn hint(path: &Path, direct: &str, sudo: &str) -> String {
    format!(
        "无法写入 {}：{}\n已尝试 sudo -n 助手：{}\n请运行 sudo script/linux/setup-sudo.sh（给 {} 免密执行 nginxwebui-priv）。不要用 sudo 启动本服务。",
        path.display(),
        direct,
        sudo.trim(),
        user_name()
    )
}

fn helper_path() -> Option<PathBuf> {
    #[cfg(not(unix))]
    {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let p = PathBuf::from("/usr/local/sbin/nginxwebui-priv");
        let meta = p.symlink_metadata().ok()?;
        if meta.file_type().is_symlink() || meta.uid() != 0 {
            return None;
        }
        p.is_file().then_some(p)
    }
}

fn sudo_output(args: &[&str], stdin_data: Option<&[u8]>) -> Result<String, String> {
    let mut cmd = Command::new("sudo");
    cmd.arg("-n");
    cmd.args(args);
    if stdin_data.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("sudo: {e}"))?;
    if let Some(data) = stdin_data {
        if let Some(mut sin) = child.stdin.take() {
            sin.write_all(data)
                .map_err(|e| format!("sudo stdin: {e}"))?;
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("sudo: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(if text.trim().is_empty() {
            "sudo -n failed".into()
        } else {
            text
        })
    }
}

fn sudo_helper(verb: &str, args: &[&str], stdin_data: Option<&[u8]>) -> Result<String, String> {
    let helper = helper_path().ok_or_else(|| "no root-owned nginxwebui-priv".to_string())?;
    let helper_s = helper.display().to_string();
    let mut full: Vec<&str> = vec![&helper_s, verb];
    full.extend_from_slice(args);
    sudo_output(&full, stdin_data)
}

pub fn run_helper(verb: &str, args: &[&str]) -> Result<String, String> {
    sudo_helper(verb, args, None)
}

pub fn sudo_n(args: &[&str]) -> Result<String, String> {
    sudo_output(args, None)
}

fn sudo_mkdir(path: &Path) -> Result<(), String> {
    let p = path_text(path);
    sudo_helper("mkdir", &[&p], None).map(|_| ())
}

fn sudo_write(path: &Path, data: &[u8]) -> Result<(), String> {
    let p = path_text(path);
    sudo_helper("write", &[&p], Some(data)).map(|_| ())
}

fn sudo_rm(path: &Path) -> Result<(), String> {
    let p = path_text(path);
    sudo_helper("rm", &[&p], None).map(|_| ())
}

#[cfg(unix)]
fn sudo_ln(from: &Path, to: &Path) -> Result<(), String> {
    let a = path_text(from);
    let b = path_text(to);
    sudo_helper("ln", &[&a, &b], None).map(|_| ())
}

pub fn mkdir_all(path: &Path) -> Result<(), String> {
    match fs::create_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if denied(&err) => {
            if cfg!(unix) {
                sudo_mkdir(path).map_err(|e| hint(path, &err.to_string(), &e))
            } else {
                Err(format!("{}: {}", path.display(), err))
            }
        }
        Err(err) => Err(format!("{}: {}", path.display(), err)),
    }
}

pub fn write(path: &Path, data: impl AsRef<[u8]>) -> Result<(), String> {
    let data = data.as_ref();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            mkdir_all(parent)?;
        }
    }
    match fs::write(path, data) {
        Ok(()) => Ok(()),
        Err(err) if denied(&err) => {
            if cfg!(unix) {
                sudo_write(path, data).map_err(|e| hint(path, &err.to_string(), &e))
            } else {
                Err(format!("{}: {}", path.display(), err))
            }
        }
        Err(err) => Err(format!("{}: {}", path.display(), err)),
    }
}

fn copy_limited<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    max: u64,
) -> Result<u64, String> {
    let mut buf = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > max {
            return Err("zipTooLarge".into());
        }
        writer.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    Ok(total)
}

/// Stream `reader` onto `path`, refusing more than `max` bytes.
pub fn write_limited<R: Read>(path: &Path, reader: &mut R, max: u64) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            mkdir_all(parent)?;
        }
    }
    match fs::File::create(path) {
        Ok(mut file) => match copy_limited(reader, &mut file, max) {
            Ok(n) => Ok(n),
            Err(err) => {
                let _ = fs::remove_file(path);
                Err(err)
            }
        },
        Err(err) if denied(&err) => {
            if cfg!(unix) {
                let mut buf = Vec::new();
                let n = copy_limited(reader, &mut buf, max)?;
                sudo_write(path, &buf).map_err(|e| hint(path, &err.to_string(), &e))?;
                Ok(n)
            } else {
                Err(format!("{}: {}", path.display(), err))
            }
        }
        Err(err) => Err(format!("{}: {}", path.display(), err)),
    }
}

pub fn remove_file(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) if denied(&err) => {
            if cfg!(unix) {
                sudo_rm(path).map_err(|e| hint(path, &err.to_string(), &e))
            } else {
                Err(format!("{}: {}", path.display(), err))
            }
        }
        Err(err) => Err(format!("{}: {}", path.display(), err)),
    }
}

pub fn link_or_copy(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        match symlink(from, to) {
            Ok(()) => return Ok(()),
            Err(err) if denied(&err) => {
                return sudo_ln(from, to).map_err(|e| hint(to, &err.to_string(), &e));
            }
            Err(_) => {}
        }
    }
    match fs::copy(from, to) {
        Ok(_) => Ok(()),
        Err(err) if denied(&err) => {
            let a = path_text(from);
            let b = path_text(to);
            sudo_helper("cp", &[&a, &b], None)
                .map(|_| ())
                .map_err(|e| hint(to, &err.to_string(), &e))
        }
        Err(err) => Err(format!("{}: {}", to.display(), err)),
    }
}

pub fn read_to_string(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if denied(&err) => {
            if cfg!(unix) {
                sudo_helper("read", &[&path_text(path)], None)
                    .map_err(|e| hint(path, &err.to_string(), &e))
            } else {
                Err(format!("{}: {}", path.display(), err))
            }
        }
        Err(err) => Err(format!("{}: {}", path.display(), err)),
    }
}

pub fn tail(path: &Path, max_bytes: u64) -> Result<String, String> {
    match fs::File::open(path) {
        Ok(mut file) => {
            let size = file.metadata().map(|m| m.len()).unwrap_or(0);
            let start = size.saturating_sub(max_bytes);
            let _ = file.seek(SeekFrom::Start(start));
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)
                .map_err(|e| format!("{}: {}", path.display(), e))?;
            Ok(String::from_utf8_lossy(&buffer).into_owned())
        }
        Err(err) if denied(&err) => {
            if cfg!(unix) {
                let n = max_bytes.clamp(1, 1024 * 1024).to_string();
                let p = path_text(path);
                sudo_helper("tail", &[&p, &n], None).map_err(|e| hint(path, &err.to_string(), &e))
            } else {
                Err(format!("{}: {}", path.display(), err))
            }
        }
        Err(err) => Err(format!("{}: {}", path.display(), err)),
    }
}

pub fn list_names(dir: &Path) -> Result<Vec<String>, String> {
    match fs::read_dir(dir) {
        Ok(entries) => Ok(entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()),
        Err(err) if denied(&err) => {
            if cfg!(unix) {
                let text = sudo_helper("ls", &[&path_text(dir)], None)
                    .map_err(|e| hint(dir, &err.to_string(), &e))?;
                Ok(text
                    .lines()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect())
            } else {
                Err(format!("{}: {}", dir.display(), err))
            }
        }
        Err(err) => Err(format!("{}: {}", dir.display(), err)),
    }
}

pub fn empty_dir(path: &Path) -> Result<(), String> {
    match fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let p = entry.path();
                let result = if p.is_dir() {
                    fs::remove_dir_all(&p)
                } else {
                    fs::remove_file(&p)
                };
                if let Err(err) = result {
                    if denied(&err) && cfg!(unix) {
                        return sudo_helper("empty-dir", &[&path_text(path)], None)
                            .map(|_| ())
                            .map_err(|e| hint(path, &err.to_string(), &e));
                    }
                    return Err(format!("{}: {}", p.display(), err));
                }
            }
            Ok(())
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) if denied(&err) => {
            if cfg!(unix) {
                sudo_helper("empty-dir", &[&path_text(path)], None)
                    .map(|_| ())
                    .map_err(|e| hint(path, &err.to_string(), &e))
            } else {
                Err(format!("{}: {}", path.display(), err))
            }
        }
        Err(err) => Err(format!("{}: {}", path.display(), err)),
    }
}

pub fn chmod(path: &Path, mode: &str) -> Result<(), String> {
    match mode {
        "644" | "640" | "600" | "755" | "750" => {}
        _ => return Err(format!("bad mode {mode}")),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mut perms = meta.permissions();
            let parsed = u32::from_str_radix(mode, 8).unwrap_or(0);
            perms.set_mode(parsed);
            if fs::set_permissions(path, perms).is_ok() {
                return Ok(());
            }
        }
    }
    if cfg!(unix) {
        let p = path_text(path);
        sudo_helper("chmod", &[&p, mode], None)
            .map(|_| ())
            .map_err(|e| hint(path, "chmod", &e))
    } else {
        Ok(())
    }
}

pub fn mkdir_www(path: &Path) -> Result<(), String> {
    match fs::create_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if denied(&err) => {
            mkdir_all(path)?;
            let user = user_name();
            if cfg!(unix) && user != "USER" && user != "root" {
                let p = path_text(path);
                let _ = sudo_helper("chown-www", &[&p, &user], None);
            }
            Ok(())
        }
        Err(err) => Err(format!("{}: {}", path.display(), err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_mkdir_in_temp() {
        let dir = std::env::temp_dir().join(format!("nginxwebui-privfs-{}", std::process::id()));
        let file = dir.join("a").join("b.conf");
        write(&file, "hello").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
        remove_file(&file).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hint_mentions_path_and_setup() {
        let text = hint(
            Path::new("/etc/nginx/sites-available/x.conf"),
            "Permission denied (os error 13)",
            "a password is required",
        );
        assert!(text.contains("/etc/nginx/sites-available/x.conf"));
        assert!(text.contains("setup-sudo.sh"));
        assert!(text.contains("Permission denied"));
        assert!(text.contains("不要用 sudo 启动"));
    }

    #[test]
    fn empty_dir_in_temp() {
        let dir = std::env::temp_dir().join(format!("nginxwebui-empty-{}", std::process::id()));
        let nested = dir.join("a");
        write(&nested.join("b.txt"), "x").unwrap();
        empty_dir(&dir).unwrap();
        assert!(dir.exists());
        assert!(fs::read_dir(&dir).unwrap().next().is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
