//! Writes that may need `sudo -n` when the process is not root.
//! Direct I/O first; on EACCES retry the root-owned helper, then raw sudo.

use std::fs;
use std::io::{ErrorKind, Write};
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
        "无法写入 {}：{}\n已尝试 sudo -n：{}\n请运行 sudo script/linux/setup-sudo.sh，或给 {} 免密 sudo，或用 root 启动本服务。",
        path.display(),
        direct,
        sudo.trim(),
        user_name()
    )
}

#[cfg(unix)]
fn is_root_owned(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    path.metadata().map(|m| m.uid() == 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_root_owned(_path: &Path) -> bool {
    false
}

fn helper_path() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("/usr/local/sbin/nginxwebui-priv")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("nginxwebui-priv"));
            candidates.push(dir.join("nginxwebui-priv.sh"));
        }
    }
    candidates.into_iter().find(|p| p.is_file() && is_root_owned(p))
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
            sin.write_all(data).map_err(|e| format!("sudo stdin: {e}"))?;
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

fn sudo_mkdir(path: &Path) -> Result<(), String> {
    let p = path_text(path);
    match sudo_helper("mkdir", &[&p], None) {
        Ok(_) => Ok(()),
        Err(_) => sudo_output(&["mkdir", "-p", &p], None).map(|_| ()),
    }
}

fn sudo_write(path: &Path, data: &[u8]) -> Result<(), String> {
    let p = path_text(path);
    match sudo_helper("write", &[&p], Some(data)) {
        Ok(_) => return Ok(()),
        Err(err) => {
            let tmp = std::env::temp_dir().join(format!(
                "nginxwebui-{}-{}.tmp",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            fs::write(&tmp, data).map_err(|e| e.to_string())?;
            let src = tmp.display().to_string();
            let result = sudo_output(&["install", "-m", "644", &src, &p], None);
            let _ = fs::remove_file(&tmp);
            result.map(|_| ()).map_err(|e2| format!("{err}; {e2}"))
        }
    }
}

fn sudo_rm(path: &Path) -> Result<(), String> {
    let p = path_text(path);
    match sudo_helper("rm", &[&p], None) {
        Ok(_) => Ok(()),
        Err(_) => sudo_output(&["rm", "-f", &p], None).map(|_| ()),
    }
}

#[cfg(unix)]
fn sudo_ln(from: &Path, to: &Path) -> Result<(), String> {
    let a = path_text(from);
    let b = path_text(to);
    match sudo_helper("ln", &[&a, &b], None) {
        Ok(_) => Ok(()),
        Err(_) => sudo_output(&["ln", "-sfn", &a, &b], None).map(|_| ()),
    }
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
            match sudo_helper("cp", &[&a, &b], None) {
                Ok(_) => Ok(()),
                Err(_) => sudo_output(&["cp", "-a", &a, &b], None)
                    .map(|_| ())
                    .map_err(|e| hint(to, &err.to_string(), &e)),
            }
        }
        Err(err) => Err(format!("{}: {}", to.display(), err)),
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
        let text = hint(Path::new("/etc/nginx/sites-available/x.conf"), "Permission denied (os error 13)", "a password is required");
        assert!(text.contains("/etc/nginx/sites-available/x.conf"));
        assert!(text.contains("setup-sudo.sh"));
        assert!(text.contains("Permission denied"));
    }
}
