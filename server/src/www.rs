//! Static site folders: unzip an uploaded zip into a web root, matching the
//! original project's www page. Paths follow `script/` (`/var/www/<domain>`).

use crate::site;
use crate::store::Db;
#[cfg(test)]
use crate::store::str_of;
use serde_json::{json, Value};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

pub const MAX_ZIP_ENTRIES: usize = 2000;
pub const MAX_ZIP_FILE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_ZIP_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

pub fn web_root(db: &Db) -> PathBuf {
    site::web_root_base(db)
}

pub fn resolve_dir(db: &Db, raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("badDir".into());
    }
    if trimmed.contains('\0') {
        return Err("badDir".into());
    }
    let path = if Path::new(trimmed).is_absolute() {
        PathBuf::from(trimmed)
    } else {
        web_root(db).join(trimmed)
    };
    if !is_allowed_dir(db, &path) {
        return Err("dirNotAllowed".into());
    }
    Ok(path)
}

fn is_allowed_dir(db: &Db, path: &Path) -> bool {
    let roots = allowed_roots(db);
    let target = normalize(path);
    roots.iter().any(|root| path_under(&target, root))
}

fn path_under(path: &Path, root: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }
    #[cfg(windows)]
    {
        let p = path.to_string_lossy().to_lowercase();
        let r = root.to_string_lossy().to_lowercase();
        return Path::new(&p).starts_with(Path::new(&r));
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn allowed_roots(db: &Db) -> Vec<PathBuf> {
    let mut roots = vec![normalize(&web_root(db))];
    let home = db.setting("home");
    if !home.is_empty() {
        roots.push(normalize(&PathBuf::from(&home).join("www")));
    }
    if crate::nginx::unix_is_dir("/var/www") {
        roots.push(normalize(Path::new("/var/www")));
    }
    roots
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = out.pop();
            }
            Component::Normal(s) => out.push(s),
        }
    }
    out
}

pub fn safe_zip_name(name: &str) -> Option<PathBuf> {
    if name.contains('\0') {
        return None;
    }
    let replaced = name.replace('\\', "/");
    if replaced.starts_with('/') || replaced.starts_with('\\') {
        return None;
    }
    if replaced.contains(':') && replaced.chars().nth(1) == Some(':') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in replaced.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

pub fn unzip_bytes(bytes: &[u8], dest: &Path) -> Result<Vec<String>, String> {
    crate::privfs::mkdir_www(dest)?;
    let dest_canon = fs::canonicalize(dest).map_err(|e| format!("{}: {}", dest.display(), e))?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err("zipTooManyFiles".into());
    }
    let mut written: Vec<String> = Vec::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        let rel = safe_zip_name(&name).ok_or_else(|| format!("unsafeZipPath: {}", name))?;
        let dest_path = dest_canon.join(&rel);
        if !dest_path.starts_with(&dest_canon) {
            return Err(format!("unsafeZipPath: {}", name));
        }
        if file.is_dir() || name.ends_with('/') {
            crate::privfs::mkdir_all(&dest_path)?;
            continue;
        }
        if let Some(parent) = dest_path.parent() {
            crate::privfs::mkdir_all(parent)?;
        }
        let n = crate::privfs::write_limited(&dest_path, &mut file, MAX_ZIP_FILE_BYTES)?;
        total = total.saturating_add(n);
        if total > MAX_ZIP_TOTAL_BYTES {
            let _ = crate::privfs::remove_file(&dest_path);
            return Err("zipTooLarge".into());
        }
        written.push(rel.display().to_string());
    }
    Ok(written)
}

pub fn clean_dir(path: &Path) -> Result<(), String> {
    crate::privfs::empty_dir(path)
}

pub fn dir_info(path: &Path) -> Value {
    let mut files = 0u64;
    let mut bytes = 0u64;
    if let Ok(walker) = walk(path) {
        for (is_file, size) in walker {
            if is_file {
                files += 1;
                bytes += size;
            }
        }
    }
    json!({
        "path": path.display().to_string(),
        "exists": path.exists(),
        "files": files,
        "bytes": bytes,
    })
}

fn walk(path: &Path) -> std::io::Result<Vec<(bool, u64)>> {
    let mut out = Vec::new();
    fn rec(path: &Path, out: &mut Vec<(bool, u64)>, depth: usize) -> std::io::Result<()> {
        if depth > 12 {
            return Ok(());
        }
        let entries = match fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                rec(&p, out, depth + 1)?;
            } else if let Ok(meta) = p.metadata() {
                out.push((true, meta.len()));
            }
        }
        Ok(())
    }
    rec(path, &mut out, 0)?;
    Ok(out)
}

/// Tiny multipart parser: fields plus one file body.
pub fn parse_multipart(body: &[u8], content_type: &str) -> Result<(Value, Vec<u8>), String> {
    let boundary = content_type
        .split(';')
        .filter_map(|p| p.trim().strip_prefix("boundary="))
        .map(|b| b.trim_matches('"').to_string())
        .next()
        .ok_or_else(|| "badMultipart".to_string())?;
    let token = format!("--{}", boundary);
    let token_bytes = token.as_bytes();
    let mut fields = serde_json::Map::new();
    let mut file = Vec::new();
    let mut start = 0;
    while let Some(rel) = find_subslice(&body[start..], token_bytes) {
        let part_start = start + rel + token_bytes.len();
        if body.get(part_start..part_start + 2) == Some(b"--") {
            break;
        }
        let after_cr = if body.get(part_start..part_start + 2) == Some(b"\r\n") {
            part_start + 2
        } else {
            part_start
        };
        let next = find_subslice(&body[after_cr..], token_bytes)
            .map(|n| after_cr + n)
            .unwrap_or(body.len());
        let mut part = &body[after_cr..next];
        if part.ends_with(b"\r\n") {
            part = &part[..part.len() - 2];
        }
        if let Some(sep) = find_subslice(part, b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&part[..sep]).to_string();
            let content = &part[sep + 4..];
            let name = header_name(&header).unwrap_or_default();
            if header.to_lowercase().contains("filename=") {
                file = content.to_vec();
                fields.insert("filename".into(), header_filename(&header).into());
            } else if !name.is_empty() {
                fields.insert(
                    name.into(),
                    String::from_utf8_lossy(content).trim_end_matches(['\r', '\n']).into(),
                );
            }
        }
        start = next;
    }
    Ok((Value::Object(fields), file))
}

fn header_name(header: &str) -> Option<String> {
    for line in header.lines() {
        for part in line.split(';') {
            let part = part.trim();
            let lower = part.to_ascii_lowercase();
            if lower.starts_with("name=") && !lower.starts_with("filename=") {
                let raw = part[5..].trim().trim_matches('"');
                if !raw.is_empty() {
                    return Some(raw.to_string());
                }
            }
        }
    }
    None
}

fn header_filename(header: &str) -> String {
    for line in header.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(idx) = lower.find("filename=") {
            let raw = line[idx + 9..].trim().trim_matches('"');
            return raw.split(';').next().unwrap_or(raw).trim_matches('"').to_string();
        }
    }
    String::new()
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[allow(dead_code)]
pub fn read_all<R: Read>(mut r: R) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zip_slip() {
        assert!(safe_zip_name("../etc/passwd").is_none());
        assert!(safe_zip_name("/etc/passwd").is_none());
        assert!(safe_zip_name("ok/index.html").is_some());
        assert_eq!(
            safe_zip_name("site/css/app.css").unwrap(),
            PathBuf::from("site/css/app.css")
        );
    }

    fn zip_bytes(name: &str, payload: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file(name, options).unwrap();
            writer.write_all(payload).unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn unzip_rejects_oversize_file() {
        let dest = std::env::temp_dir().join(format!("nginxwebui-zip-oversize-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        fs::create_dir_all(&dest).unwrap();
        let payload = vec![b'x'; (MAX_ZIP_FILE_BYTES as usize) + 1];
        let bytes = zip_bytes("huge.txt", &payload);
        let err = unzip_bytes(&bytes, &dest).unwrap_err();
        assert!(err.contains("zipTooLarge"), "{err}");
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn unzip_writes_small_file() {
        let dest = std::env::temp_dir().join(format!("nginxwebui-zip-ok-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        fs::create_dir_all(&dest).unwrap();
        let bytes = zip_bytes("ok.txt", b"hello");
        let written = unzip_bytes(&bytes, &dest).unwrap();
        assert_eq!(written, vec!["ok.txt".to_string()]);
        assert_eq!(fs::read_to_string(dest.join("ok.txt")).unwrap(), "hello");
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn parses_multipart_fields() {
        let body = b"--abc\r\nContent-Disposition: form-data; name=\"dir\"\r\n\r\n/var/www/demo\r\n--abc\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.zip\"\r\nContent-Type: application/zip\r\n\r\nPK\x03\x04xx\r\n--abc--\r\n";
        let (fields, file) = parse_multipart(body, "multipart/form-data; boundary=abc").unwrap();
        assert_eq!(str_of(&fields, "dir"), "/var/www/demo");
        assert_eq!(file, b"PK\x03\x04xx");
    }
}
