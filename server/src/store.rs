//! SQLite-backed store. Rows stay as JSON documents in one `records` table so
//! the rest of the server can keep using a small in-memory `Db`. WAL mode
//! gives crash-safe writes without a database server process.

use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const TABLES: [&str; 19] = [
    "basic",
    "http",
    "server",
    "location",
    "upstream",
    "upstreamServer",
    "stream",
    "param",
    "template",
    "denyAllow",
    "password",
    "www",
    "cert",
    "site",
    "setting",
    "admin",
    "bak",
    "bakSub",
    "operateLog",
];

#[derive(Clone, Default)]
pub struct Db {
    pub t: BTreeMap<String, Vec<Value>>,
}

impl Db {
    pub fn new() -> Db {
        let mut t = BTreeMap::new();
        for name in TABLES {
            t.insert(name.to_string(), Vec::new());
        }
        Db { t }
    }

    pub fn rows(&self, table: &str) -> &[Value] {
        match self.t.get(table) {
            Some(v) => v.as_slice(),
            None => &[],
        }
    }

    pub fn rows_mut(&mut self, table: &str) -> &mut Vec<Value> {
        self.t.entry(table.to_string()).or_default()
    }

    pub fn get(&self, table: &str, id: &str) -> Option<Value> {
        self.rows(table)
            .iter()
            .find(|r| str_of(r, "id") == id)
            .cloned()
    }

    /// Insert when `id` is missing or unknown, otherwise replace that row.
    pub fn upsert(&mut self, table: &str, mut row: Value) -> String {
        let mut id = str_of(&row, "id");
        if id.is_empty() {
            id = new_id();
        }
        set_str(&mut row, "id", &id);
        let rows = self.rows_mut(table);
        if let Some(slot) = rows.iter_mut().find(|r| str_of(r, "id") == id) {
            *slot = row;
        } else {
            rows.push(row);
        }
        id
    }

    pub fn delete(&mut self, table: &str, id: &str) {
        self.rows_mut(table).retain(|r| str_of(r, "id") != id);
    }

    /// Remove rows of `table` whose `column` equals `value`.
    pub fn delete_where(&mut self, table: &str, column: &str, value: &str) {
        self.rows_mut(table).retain(|r| str_of(r, column) != value);
    }

    pub fn setting(&self, key: &str) -> String {
        self.rows("setting")
            .iter()
            .find(|r| str_of(r, "key") == key)
            .map(|r| str_of(r, "value"))
            .unwrap_or_default()
    }

    pub fn set_setting(&mut self, key: &str, value: &str) {
        let rows = self.rows_mut("setting");
        if let Some(row) = rows.iter_mut().find(|r| str_of(r, "key") == key) {
            set_str(row, "value", value);
        } else {
            let mut m = Map::new();
            m.insert("id".into(), Value::String(new_id()));
            m.insert("key".into(), Value::String(key.to_string()));
            m.insert("value".into(), Value::String(value.to_string()));
            rows.push(Value::Object(m));
        }
    }
}

pub struct Store {
    #[allow(dead_code)]
    pub dir: PathBuf,
    pub db_path: PathBuf,
    pub db: Db,
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(dir: &Path) -> Store {
        let _ = fs::create_dir_all(dir);
        let db_path = dir.join("nginxwebui.db");
        let conn = open_sqlite(&db_path);
        let mut db = load_from_sqlite(&conn);

        let json_path = dir.join("db.json");
        if db.rows("setting").is_empty() && db.rows("admin").is_empty() && json_path.is_file() {
            if let Ok(bytes) = fs::read(&json_path) {
                if let Ok(mut t) = serde_json::from_slice::<BTreeMap<String, Vec<Value>>>(&bytes) {
                    for name in TABLES {
                        t.entry(name.to_string()).or_default();
                    }
                    db = Db { t };
                }
            }
        }

        let store = Store {
            dir: dir.to_path_buf(),
            db_path,
            db,
            conn: Mutex::new(conn),
        };
        let _ = store.flush();
        store
    }

    /// Persist in-memory rows with INSERT OR REPLACE and delete orphans.
    /// Keeps the existing `records(tbl, id, data)` schema.
    pub fn flush(&self) -> std::io::Result<()> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io_err)?;
        {
            let mut upsert = tx
                .prepare(
                    "INSERT INTO records (tbl, id, data) VALUES (?1, ?2, ?3)
                     ON CONFLICT(tbl, id) DO UPDATE SET data = excluded.data",
                )
                .map_err(io_err)?;
            let mut keep: HashSet<(String, String)> = HashSet::new();
            for (table, rows) in &self.db.t {
                for row in rows {
                    let id = str_of(row, "id");
                    if id.is_empty() {
                        continue;
                    }
                    let data = serde_json::to_string(row).unwrap_or_else(|_| "{}".into());
                    upsert.execute(params![table, id, data]).map_err(io_err)?;
                    keep.insert((table.clone(), id));
                }
            }
            drop(upsert);

            let existing: Vec<(String, String)> = {
                let mut stmt = tx.prepare("SELECT tbl, id FROM records").map_err(io_err)?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(io_err)?;
                rows.flatten().collect()
            };
            let mut delete = tx
                .prepare("DELETE FROM records WHERE tbl = ?1 AND id = ?2")
                .map_err(io_err)?;
            for (table, id) in existing {
                if !keep.contains(&(table.clone(), id.clone())) {
                    delete.execute(params![table, id]).map_err(io_err)?;
                }
            }
        }
        tx.commit().map_err(io_err)?;
        Ok(())
    }
}

fn open_sqlite(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap_or_else(|err| {
        panic!("cannot open sqlite {}: {}", path.display(), err);
    });
    let _ = conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS records (
           tbl TEXT NOT NULL,
           id TEXT NOT NULL,
           data TEXT NOT NULL,
           PRIMARY KEY (tbl, id)
         );",
    );
    conn
}

fn load_from_sqlite(conn: &Connection) -> Db {
    let mut db = Db::new();
    let mut stmt = match conn.prepare("SELECT tbl, id, data FROM records") {
        Ok(stmt) => stmt,
        Err(_) => return db,
    };
    let rows = stmt.query_map([], |row| {
        let tbl: String = row.get(0)?;
        let data: String = row.get(2)?;
        Ok((tbl, data))
    });
    if let Ok(rows) = rows {
        for row in rows.flatten() {
            if let Ok(value) = serde_json::from_str::<Value>(&row.1) {
                db.rows_mut(&row.0).push(value);
            }
        }
    }
    db
}

fn io_err(err: rusqlite::Error) -> std::io::Error {
    std::io::Error::other(err.to_string())
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn new_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed) % 1000;
    format!("{}{:03}", ms, n)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---- reading JSON fields without caring about their exact type -------------

pub fn str_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

pub fn int_of(v: &Value, key: &str) -> i64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        Some(Value::Bool(b)) => {
            if *b {
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

pub fn bool_of(v: &Value, key: &str, default: bool) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !(s == "false" || s == "0" || s.is_empty()),
        Some(Value::Number(n)) => n.as_i64().unwrap_or(1) != 0,
        Some(Value::Null) => default,
        None => default,
        _ => default,
    }
}

pub fn set_str(v: &mut Value, key: &str, val: &str) {
    if let Value::Object(m) = v {
        m.insert(key.to_string(), Value::String(val.to_string()));
    }
}

/// Rows of a table, ordered by `seq` (smallest first when `asc`), newest first
/// on ties, the way the Java version's `Sort` behaved.
pub fn ordered(db: &Db, table: &str, asc: bool) -> Vec<Value> {
    let mut list: Vec<Value> = db.rows(table).to_vec();
    list.sort_by(|a, b| {
        let (sa, sb) = (int_of(a, "seq"), int_of(b, "seq"));
        let ord = sa.cmp(&sb);
        let ord = if asc { ord } else { ord.reverse() };
        if ord == std::cmp::Ordering::Equal {
            str_of(b, "id").cmp(&str_of(a, "id"))
        } else {
            ord
        }
    });
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::thread;

    fn temp_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-store-{}-{}-{}",
            label,
            std::process::id(),
            new_id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Store::open(&dir)
    }

    #[test]
    fn flush_keeps_updates_and_deletes_without_full_wipe() {
        let mut store = temp_store("upsert");
        store.db.set_setting("hello", "world");
        let id = store.db.upsert("site", json!({ "name": "a" }));
        store.flush().unwrap();

        store.db.set_setting("hello", "there");
        store.db.delete("site", &id);
        let id2 = store.db.upsert("site", json!({ "name": "b" }));
        store.flush().unwrap();

        let reopened = Store::open(&store.dir);
        assert_eq!(reopened.db.setting("hello"), "there");
        assert!(reopened.db.get("site", &id).is_none());
        assert_eq!(str_of(&reopened.db.get("site", &id2).unwrap(), "name"), "b");
        assert_eq!(reopened.db.rows("site").len(), 1);
    }

    #[test]
    fn concurrent_flushes_do_not_corrupt_rows() {
        let mut store = temp_store("conc");
        for i in 0..24 {
            store.db.upsert(
                "site",
                json!({ "id": format!("s{i}"), "name": format!("n{i}") }),
            );
        }
        store.flush().unwrap();
        let shared = Arc::new(StdMutex::new(store));
        let mut handles = Vec::new();
        for t in 0..6 {
            let shared = shared.clone();
            handles.push(thread::spawn(move || {
                for i in 0..24 {
                    let mut guard = shared.lock().unwrap();
                    let id = format!("s{i}");
                    if let Some(mut row) = guard.db.get("site", &id) {
                        if let Value::Object(map) = &mut row {
                            map.insert("name".into(), format!("t{t}-{i}").into());
                        }
                        guard.db.upsert("site", row);
                    }
                    guard.flush().unwrap();
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let guard = shared.lock().unwrap();
        let dir = guard.dir.clone();
        drop(guard);
        let reopened = Store::open(&dir);
        assert_eq!(reopened.db.rows("site").len(), 24);
        for i in 0..24 {
            let row = reopened.db.get("site", &format!("s{i}")).unwrap();
            let name = str_of(&row, "name");
            assert!(name.contains(&format!("-{i}")), "{name}");
        }
    }
}
