//! Agent auto-upgrade from the configured panel.
//!
//! The agent only downloads `{panel}/agent/download?os=&arch=`, never a
//! third-party URL. The payload is checked against the SHA-256 advertised in
//! the hello reply. Default: enabled on verified HTTPS or loopback HTTP.

use crate::tls;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Decision {
    SkipCurrent,
    SkipDisabled,
    SkipInsecureTransport,
    SkipNoArtifact,
    Need {
        version: String,
        sha256: String,
        url: String,
    },
}

pub fn decide(
    enabled: bool,
    panel_url: &str,
    current_version: &str,
    current_sha256: Option<&str>,
    advertised: &Value,
) -> Decision {
    if !enabled {
        return Decision::SkipDisabled;
    }
    if !tls::upgrade_transport_ok(panel_url) {
        return Decision::SkipInsecureTransport;
    }
    let version = crate::store::str_of(advertised, "version");
    let sha256 = crate::store::str_of(advertised, "sha256");
    if sha256.is_empty() {
        return Decision::SkipNoArtifact;
    }
    if !version.is_empty() && version == current_version {
        return Decision::SkipCurrent;
    }
    if let Some(local) = current_sha256 {
        if !local.is_empty() && hashes_equal(local, &sha256) {
            return Decision::SkipCurrent;
        }
    }
    let base = panel_url.trim_end_matches('/');
    let os = advertised
        .get("os")
        .and_then(|v| v.as_str())
        .unwrap_or(std::env::consts::OS);
    let arch = advertised
        .get("arch")
        .and_then(|v| v.as_str())
        .unwrap_or(std::env::consts::ARCH);
    Decision::Need {
        version,
        sha256,
        url: format!("{base}/agent/download?os={os}&arch={arch}"),
    }
}

pub fn hashes_equal(a: &str, b: &str) -> bool {
    match (tls::parse_fingerprint(a), tls::parse_fingerprint(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.eq_ignore_ascii_case(b),
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    tls::fingerprint_hex(&Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(sha256_hex(&bytes))
}

pub fn verify_bytes(bytes: &[u8], expected: &str) -> Result<(), String> {
    let got = sha256_hex(bytes);
    if hashes_equal(&got, expected) {
        Ok(())
    } else {
        Err(format!(
            "agent download hash mismatch (got {got}, expected {expected})"
        ))
    }
}

pub fn current_binary_sha256() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    sha256_file(&exe)
}

/// Write `bytes` to `dest` atomically (temp file + rename) and chmod 0755.
pub fn replace_binary(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let tmp = parent.join(format!(
        ".{}.upgrade.tmp",
        dest.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("nginxwebui-agent")
    ));
    let _ = std::fs::remove_file(&tmp);
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o755);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|err| format!("cannot write upgrade temp: {err}"))?;
        file.write_all(bytes)
            .map_err(|err| format!("cannot write upgrade temp: {err}"))?;
        file.sync_all()
            .map_err(|err| format!("cannot sync upgrade temp: {err}"))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&tmp, dest).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot install new agent at {}: {err}", dest.display())
    })
}

pub fn download(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("upgrade url must be on the configured panel".into());
    }
    let response = agent
        .get(url)
        .call()
        .map_err(|err| format!("upgrade download failed: {err}"))?;
    if response.status() != 200 {
        return Err(format!("upgrade download HTTP {}", response.status()));
    }
    let mut buf = Vec::new();
    response
        .into_reader()
        .take(80 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|err| format!("upgrade download: {err}"))?;
    if buf.is_empty() {
        return Err("upgrade download was empty".into());
    }
    Ok(buf)
}

/// Replace this process with the binary at `dest`, keeping the original argv.
pub fn reexec(dest: &Path) -> Result<(), String> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(dest).args(&args).exec();
        Err(format!("re-exec failed: {err}"))
    }
    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(dest)
            .args(&args)
            .status()
            .map_err(|err| format!("re-exec failed: {err}"))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn skips_when_already_current_or_disabled() {
        let adv =
            json!({"version": "1.0.0", "sha256": "aa".repeat(32), "os": "linux", "arch": "x86_64"});
        match decide(false, "https://panel.example", "1.0.0", None, &adv) {
            Decision::SkipDisabled => {}
            other => panic!("disabled: {other:?}"),
        }
        match decide(true, "https://panel.example", "1.0.0", None, &adv) {
            Decision::SkipCurrent => {}
            other => panic!("same version: {other:?}"),
        }
        match decide(true, "http://10.0.0.2:8080", "0.9.0", None, &adv) {
            Decision::SkipInsecureTransport => {}
            other => panic!("insecure: {other:?}"),
        }
        match decide(true, "http://127.0.0.1:8080", "1.0.0", None, &adv) {
            Decision::SkipCurrent => {}
            other => panic!("loopback current: {other:?}"),
        }
    }

    #[test]
    fn need_when_version_differs() {
        let sha = "ab".repeat(32);
        let adv = json!({"version": "1.1.0", "sha256": sha, "os": "linux", "arch": "aarch64"});
        match decide(true, "https://panel.example", "1.0.0", None, &adv) {
            Decision::Need {
                version,
                url,
                sha256,
            } => {
                assert_eq!(version, "1.1.0");
                assert_eq!(sha256, sha);
                assert_eq!(
                    url,
                    "https://panel.example/agent/download?os=linux&arch=aarch64"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn hash_mismatch_is_refused() {
        let bytes = b"new-agent-bytes";
        let good = sha256_hex(bytes);
        verify_bytes(bytes, &good).unwrap();
        let err = verify_bytes(bytes, &"00".repeat(32)).unwrap_err();
        assert!(err.contains("hash mismatch"), "{err}");
    }

    #[test]
    fn already_current_by_hash_even_if_version_differs() {
        let sha = sha256_hex(b"same-bin");
        let adv = json!({"version": "9.9.9", "sha256": sha, "os": "linux", "arch": "x86_64"});
        match decide(true, "https://panel.example", "1.0.0", Some(&sha), &adv) {
            Decision::SkipCurrent => {}
            other => panic!("{other:?}"),
        }
    }
}
