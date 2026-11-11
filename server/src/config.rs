//! Process-level settings from `nginxwebui.conf`.
//!
//! Key = value, `#` comments. Relative paths are resolved against the file's
//! directory. CLI flags and environment variables still override the file.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileValues {
    pub port: Option<u16>,
    pub home: Option<PathBuf>,
    pub web: Option<PathBuf>,
    pub threads: Option<usize>,
    pub reload_cmd: Option<String>,
    pub bind: Option<String>,
    pub secure_cookie: Option<bool>,
    pub bin: Option<PathBuf>,
    pub log: Option<PathBuf>,
    pub pid: Option<PathBuf>,
}

pub fn load(path: &Path) -> Result<FileValues, String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {}", path.display(), err))?;
    let base = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    parse(&text, &base)
}

pub fn parse(text: &str, base: &Path) -> Result<FileValues, String> {
    let mut values = FileValues::default();
    for (index, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected key = value", index + 1));
        };
        let key = normalize_key(key);
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        match key.as_str() {
            "port" | "server.port" | "server_port" => {
                values.port = Some(
                    value
                        .parse()
                        .map_err(|_| format!("line {}: invalid port", index + 1))?,
                );
            }
            "home" | "project.home" | "project_home" => {
                values.home = Some(resolve(base, &value));
            }
            "web_dir" | "web.dir" | "web" => {
                values.web = Some(resolve(base, &value));
            }
            "threads" => {
                values.threads = Some(
                    value
                        .parse()
                        .map_err(|_| format!("line {}: invalid threads", index + 1))?,
                );
            }
            "reload_cmd" | "reload.cmd" | "reloadcmd" => {
                values.reload_cmd = Some(value);
            }
            "bind" | "listen" | "listen_address" | "server.bind" | "server_bind" => {
                values.bind = Some(value);
            }
            "secure_cookie" | "secure.cookie" | "secure_cookies" => {
                values.secure_cookie = Some(
                    parse_bool(&value)
                        .map_err(|_| format!("line {}: invalid secure_cookie", index + 1))?,
                );
            }
            "bin" => values.bin = Some(resolve(base, &value)),
            "log" => values.log = Some(resolve(base, &value)),
            "pid" => values.pid = Some(resolve(base, &value)),
            "start_nginx" => {}
            other => {
                return Err(format!("line {}: unknown key `{other}`", index + 1));
            }
        }
    }
    Ok(values)
}

/// `--config`, `NGINXWEBUI_CONFIG`, then well-known locations.
pub fn find(cli: Option<&Path>, extra: &[PathBuf]) -> Option<PathBuf> {
    if let Some(path) = cli {
        return Some(path.to_path_buf());
    }
    if let Ok(path) = std::env::var("NGINXWEBUI_CONFIG") {
        let path = path.trim();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    let mut candidates: Vec<PathBuf> = extra.to_vec();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("nginxwebui.conf"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("nginxwebui.conf"));
            candidates.push(dir.join("..").join("nginxwebui.conf"));
            candidates.push(dir.join("..").join("..").join("nginxwebui.conf"));
        }
    }
    candidates.push(PathBuf::from("/etc/nginxwebui.conf"));
    candidates.into_iter().find(|p| p.is_file())
}

pub fn cli_config_path(args: &[String]) -> Option<PathBuf> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--config" || arg == "-c" {
            return args.get(index + 1).map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(value));
        }
        if let Some(value) = arg.strip_prefix("-c=") {
            return Some(PathBuf::from(value));
        }
        index += 1;
    }
    None
}

fn parse_bool(value: &str) -> Result<bool, ()> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(()),
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_quote = false;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' | '\'' if !in_quote || line.as_bytes().get(i.saturating_sub(1)) != Some(&b'\\') => {
                in_quote = !in_quote;
            }
            '#' if !in_quote => return &line[..i],
            _ => {}
        }
    }
    line
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn normalize_key(key: &str) -> String {
    key.trim().to_ascii_lowercase().replace('-', "_")
}

fn resolve(base: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commented_file_and_relative_paths() {
        let dir = std::env::temp_dir().join("nginxwebui-conf-test");
        let _ = std::fs::create_dir_all(&dir);
        let text = r#"
# listen
port = 9090
bind = 127.0.0.1
secure_cookie = false
home = data
web_dir = "web/build"
threads = 4
reload_cmd = nginx -s reload
"#;
        let values = parse(text, &dir).unwrap();
        assert_eq!(values.port, Some(9090));
        assert_eq!(values.bind.as_deref(), Some("127.0.0.1"));
        assert_eq!(values.secure_cookie, Some(false));
        assert_eq!(values.home.as_deref(), Some(dir.join("data").as_path()));
        assert_eq!(values.web.as_deref(), Some(dir.join("web/build").as_path()));
        assert_eq!(values.threads, Some(4));
        assert_eq!(values.reload_cmd.as_deref(), Some("nginx -s reload"));
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = parse("foo = bar\n", Path::new("/tmp")).unwrap_err();
        assert!(err.contains("unknown key"));
    }

    #[test]
    fn parses_bind_aliases() {
        let values = parse("listen = 0.0.0.0\n", Path::new("/tmp")).unwrap();
        assert_eq!(values.bind.as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn cli_config_accepts_equals_and_space() {
        let args = vec!["--config=/etc/nginxwebui.conf".into()];
        assert_eq!(
            cli_config_path(&args),
            Some(PathBuf::from("/etc/nginxwebui.conf"))
        );
        let args = vec!["-c".into(), "nginxwebui.conf".into()];
        assert_eq!(
            cli_config_path(&args),
            Some(PathBuf::from("nginxwebui.conf"))
        );
    }
}
