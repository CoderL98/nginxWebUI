//! Certificate files under a node's `sslDir`.
//!
//! Used by complete sync and the allowlisted `ssl.list` / `ssl.get` / `ssl.put`
//! RPC methods. Filenames are sanitized: no `..`, no path separators. File
//! contents are secrets and must not appear in previews, list endpoints, or
//! operate-log details.

use crate::api::{fail, ok, Ctx, Reply, Req};
use crate::store::{str_of, Db};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

pub const MAX_FILE_BYTES: u64 = 256 * 1024;
pub const MAX_FILES: usize = 64;

const SUFFIXES: &[&str] = &[".cer", ".crt", ".fullchain.cer", ".key", ".pem"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SslFile {
    pub name: String,
    pub content: String,
    pub encoding: String,
}

impl SslFile {
    pub fn from_bytes(name: String, bytes: &[u8]) -> Self {
        if let Ok(text) = std::str::from_utf8(bytes) {
            SslFile {
                name,
                content: text.to_string(),
                encoding: String::new(),
            }
        } else {
            SslFile {
                name,
                content: bytes_to_hex(bytes),
                encoding: "hex".into(),
            }
        }
    }

    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        if self.encoding == "hex" {
            parse_hex(&self.content)
        } else {
            Ok(self.content.as_bytes().to_vec())
        }
    }

    pub fn to_wire(&self) -> Value {
        let mut obj = json!({ "name": self.name, "content": self.content });
        if !self.encoding.is_empty() {
            obj["encoding"] = json!(self.encoding);
        }
        obj
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err("badHex".into());
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).map_err(|_| "badHex".into()))
        .collect()
}

/// A file name that may live directly under `sslDir`. No directories, no `..`.
pub fn sanitize_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.len() > 255 {
        return None;
    }
    if name.contains('\0') || name.contains("..") {
        return None;
    }
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return None;
    }
    let path = Path::new(name);
    if path.is_absolute() || path.components().count() != 1 {
        return None;
    }
    match path.components().next()? {
        Component::Normal(part) => {
            let text = part.to_str()?;
            if text != name {
                return None;
            }
        }
        _ => return None,
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '*'));
    if !ok {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    if !SUFFIXES.iter().any(|s| lower.ends_with(s)) {
        return None;
    }
    Some(name.to_string())
}

fn ssl_join(dir: &Path, name: &str) -> Option<PathBuf> {
    let name = sanitize_name(name)?;
    let path = dir.join(&name);
    let parent = path.parent()?;
    if parent != dir {
        return None;
    }
    Some(path)
}

fn looks_like_key(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".key") || lower.ends_with("-key.pem") || lower.ends_with("_key.pem")
}

fn stems_for(cert_name: &str) -> Vec<String> {
    let mut stems = Vec::new();
    let push = |stems: &mut Vec<String>, s: String| {
        if !s.is_empty() && !stems.iter().any(|x| x == &s) {
            stems.push(s);
        }
    };
    let name = cert_name.trim();
    if name.is_empty() {
        return stems;
    }
    push(&mut stems, name.to_string());
    if let Some(rest) = name.strip_suffix(".fullchain") {
        push(&mut stems, rest.to_string());
    }
    for suffix in [".cer", ".crt", ".pem", ".key"] {
        if let Some(rest) = name.strip_suffix(suffix) {
            push(&mut stems, rest.to_string());
        }
    }
    stems
}

fn candidate_names(cert_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for stem in stems_for(cert_name) {
        for suffix in SUFFIXES {
            let name = format!("{stem}{suffix}");
            if sanitize_name(&name).is_some() && !out.iter().any(|x| x == &name) {
                out.push(name);
            }
        }
    }
    out
}

fn read_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err(format!("sslFileTooLarge:{}", path.display()));
            }
            Ok(Some(bytes))
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            match crate::privfs::read_to_string(path) {
                Ok(text) => {
                    if text.len() as u64 > MAX_FILE_BYTES {
                        return Err(format!("sslFileTooLarge:{}", path.display()));
                    }
                    Ok(Some(text.into_bytes()))
                }
                Err(_) => Ok(None),
            }
        }
        Err(_) => Ok(None),
    }
}

/// Collect cert/key files that live in `sslDir` for the given cert names.
pub fn collect_for_names(db: &Db, cert_names: &[String]) -> (Vec<SslFile>, Vec<String>) {
    let dir = crate::site::ssl_dir(db);
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for cert_name in cert_names {
        for name in candidate_names(cert_name) {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(path) = ssl_join(&dir, &name) else {
                continue;
            };
            match read_file(&path) {
                Ok(Some(bytes)) => {
                    if files.len() >= MAX_FILES {
                        warnings.push("sslTooManyFiles".into());
                        return (files, warnings);
                    }
                    files.push(SslFile::from_bytes(name, &bytes));
                }
                Ok(None) => {}
                Err(err) => warnings.push(err),
            }
        }
    }
    (files, warnings)
}

pub fn list_names(db: &Db) -> Result<Vec<String>, String> {
    let dir = crate::site::ssl_dir(db);
    let mut names = Vec::new();
    let entries = match crate::privfs::list_names(&dir) {
        Ok(list) => list,
        Err(_) => match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .flatten()
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .collect(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.to_string()),
        },
    };
    for name in entries {
        if sanitize_name(&name).is_some() {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

pub fn get_files(db: &Db, names: &[String]) -> Result<Vec<SslFile>, String> {
    if names.len() > MAX_FILES {
        return Err("sslTooManyFiles".into());
    }
    let dir = crate::site::ssl_dir(db);
    let mut files = Vec::new();
    for raw in names {
        let Some(name) = sanitize_name(raw) else {
            return Err("badSslName".into());
        };
        let Some(path) = ssl_join(&dir, &name) else {
            return Err("badSslName".into());
        };
        match read_file(&path)? {
            Some(bytes) => files.push(SslFile::from_bytes(name, &bytes)),
            None => return Err(format!("missingSslFile:{name}")),
        }
    }
    Ok(files)
}

/// Write files under `sslDir` only. Existing files are overwritten.
pub fn put_files(db: &Db, files: &[SslFile]) -> Result<Vec<String>, String> {
    if files.len() > MAX_FILES {
        return Err("sslTooManyFiles".into());
    }
    let dir = crate::site::ssl_dir(db);
    crate::privfs::mkdir_all(&dir)?;
    let mut written = Vec::new();
    for file in files {
        let Some(name) = sanitize_name(&file.name) else {
            return Err("badSslName".into());
        };
        let Some(path) = ssl_join(&dir, &name) else {
            return Err("badSslName".into());
        };
        let bytes = file.bytes()?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("sslFileTooLarge:{name}"));
        }
        crate::privfs::write(&path, &bytes)?;
        let mode = if looks_like_key(&name) { "600" } else { "644" };
        let _ = crate::privfs::chmod(&path, mode);
        written.push(name);
    }
    Ok(written)
}

pub fn files_from_value(value: &Value) -> Result<Vec<SslFile>, String> {
    let items = value.as_array().ok_or_else(|| "badSslFiles".to_string())?;
    if items.len() > MAX_FILES {
        return Err("sslTooManyFiles".into());
    }
    let mut files = Vec::new();
    for item in items {
        let name = str_of(item, "name");
        let Some(name) = sanitize_name(&name) else {
            return Err("badSslName".into());
        };
        let content = str_of(item, "content");
        let encoding = str_of(item, "encoding");
        files.push(SslFile {
            name,
            content,
            encoding,
        });
    }
    Ok(files)
}

pub fn preview_files(files: &[SslFile]) -> Value {
    let list: Vec<Value> = files
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "bytes": f.bytes().map(|b| b.len()).unwrap_or(f.content.len()),
            })
        })
        .collect();
    json!(list)
}

fn names_of(body: &Value) -> Vec<String> {
    match body.get("names").or_else(|| body.get("name")) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

pub fn handle(ctx: &Ctx, action: &str, req: &Req) -> Reply {
    let body = if req.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&req.body).unwrap_or(Value::Null)
    };
    let store = ctx.store.lock().unwrap();
    match action {
        "list" => match list_names(&store.db) {
            Ok(names) => ok(json!({ "names": names })),
            Err(err) => fail(&err),
        },
        "get" => {
            let names = names_of(&body);
            if names.is_empty() {
                if let Some(name) = req.query.get("name") {
                    return match get_files(&store.db, std::slice::from_ref(name)) {
                        Ok(files) => ok(json!({
                            "files": files.iter().map(SslFile::to_wire).collect::<Vec<_>>()
                        })),
                        Err(err) => fail(&err),
                    };
                }
                return fail("badSslName");
            }
            match get_files(&store.db, &names) {
                Ok(files) => ok(json!({
                    "files": files.iter().map(SslFile::to_wire).collect::<Vec<_>>()
                })),
                Err(err) => fail(&err),
            }
        }
        "put" => {
            drop(store);
            let files = match body
                .get("files")
                .ok_or_else(|| "badSslFiles".to_string())
                .and_then(files_from_value)
            {
                Ok(f) => f,
                Err(err) => return fail(&err),
            };
            let store = ctx.store.lock().unwrap();
            match put_files(&store.db, &files) {
                Ok(written) => ok(json!({ "written": written, "count": written.len() })),
                Err(err) => fail(&err),
            }
        }
        _ => fail("not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db_at(label: &str) -> (std::path::PathBuf, Db) {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-ssl-{label}-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let ssl = dir.join("ssl");
        std::fs::create_dir_all(&ssl).unwrap();
        let mut db = Db::new();
        db.set_setting("sslDir", &ssl.display().to_string());
        (dir, db)
    }

    #[test]
    fn sanitize_rejects_traversal_and_separators() {
        assert_eq!(
            sanitize_name("example.com.cer").as_deref(),
            Some("example.com.cer")
        );
        assert_eq!(
            sanitize_name("example.com.fullchain.cer").as_deref(),
            Some("example.com.fullchain.cer")
        );
        assert_eq!(
            sanitize_name("star-example.com.key").as_deref(),
            Some("star-example.com.key")
        );
        assert!(sanitize_name("../etc/passwd").is_none());
        assert!(sanitize_name("..\\windows\\system32\\x.key").is_none());
        assert!(sanitize_name("foo/../bar.cer").is_none());
        assert!(sanitize_name("/etc/nginx/ssl/a.cer").is_none());
        assert!(sanitize_name("C:\\nginx\\conf\\ssl\\a.cer").is_none());
        assert!(sanitize_name("a.cer/../../x").is_none());
        assert!(sanitize_name("notes.txt").is_none());
        assert!(sanitize_name("").is_none());
        assert!(sanitize_name("a.cer\0.cer").is_none());
    }

    #[test]
    fn put_rejects_path_traversal_and_writes_only_under_ssldir() {
        let (dir, db) = db_at("put");
        let ssl = PathBuf::from(db.setting("sslDir"));
        let err = put_files(
            &db,
            &[SslFile {
                name: "../escape.cer".into(),
                content: "secret".into(),
                encoding: String::new(),
            }],
        )
        .unwrap_err();
        assert_eq!(err, "badSslName");
        assert!(!dir.join("escape.cer").exists());
        let written = put_files(
            &db,
            &[SslFile {
                name: "app.example.com.cer".into(),
                content: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into(),
                encoding: String::new(),
            }],
        )
        .unwrap();
        assert_eq!(written, vec!["app.example.com.cer"]);
        assert!(ssl.join("app.example.com.cer").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_and_get_roundtrip_without_leaking_in_preview() {
        let (dir, db) = db_at("get");
        let ssl = PathBuf::from(db.setting("sslDir"));
        std::fs::write(ssl.join("copy.example.com.cer"), "CER-SECRET").unwrap();
        std::fs::write(ssl.join("copy.example.com.key"), "KEY-SECRET").unwrap();
        let (files, warnings) = collect_for_names(&db, &["copy.example.com".into()]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(files.len(), 2);
        let preview = preview_files(&files).to_string();
        assert!(!preview.contains("CER-SECRET"), "{preview}");
        assert!(!preview.contains("KEY-SECRET"), "{preview}");
        let got = get_files(&db, &["copy.example.com.key".into()]).unwrap();
        assert_eq!(got[0].content, "KEY-SECRET");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
