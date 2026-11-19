use anyhow::{Context, Result, bail, ensure};
use chrono::{Local, NaiveDateTime, TimeZone};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct Chat {
    pub id: Option<String>,
    pub revision: i64,
    pub messages: Vec<Value>,
    pub dirty: bool,
}
#[derive(Clone)]
pub struct Entry {
    pub id: String,
    pub workspace: PathBuf,
    pub preview: String,
    pub saved: i64,
    pub count: i64,
}
pub struct Store {
    db: Connection,
    dir: PathBuf,
}
fn key(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) {
        s.replace('/', "\\").to_lowercase()
    } else {
        s.into_owned()
    }
}
fn stamp(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}
fn legacy_name(path: &Path) -> bool {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let (hash, date) = name.split_once('_').unwrap_or((name, ""));
    hash.len() == 16
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
        && (date.is_empty()
            || (date.len() == 15
                && date.as_bytes()[8] == b'_'
                && date
                    .bytes()
                    .enumerate()
                    .all(|(i, b)| i == 8 || b.is_ascii_digit())))
}
fn preview(msgs: &[Value]) -> String {
    msgs.iter()
        .find(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())
        .unwrap_or("")
        .split_whitespace()
        .take(32)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect()
}
impl Store {
    pub fn open() -> Result<Self> {
        let base = env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("LocalAssistant"))
            .or_else(|| {
                env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state/local-assistant"))
            })
            .context("Cannot locate the user profile")?;
        let dir = base.join("sessions");
        fs::create_dir_all(&dir)?;
        let db = Connection::open(dir.join("history.sqlite3"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA secure_delete=ON;
            CREATE TABLE IF NOT EXISTS chats(id TEXT PRIMARY KEY,workspace_key TEXT NOT NULL,workspace TEXT NOT NULL,saved_ns INTEGER NOT NULL,revision INTEGER NOT NULL,preview TEXT NOT NULL,message_count INTEGER NOT NULL,model TEXT NOT NULL,messages TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS chats_by_time ON chats(saved_ns DESC,id);
            CREATE INDEX IF NOT EXISTS chats_by_workspace ON chats(workspace_key,saved_ns DESC,id);
            CREATE TABLE IF NOT EXISTS active(workspace_key TEXT PRIMARY KEY,chat_id TEXT);")?;
        let mut store = Self { db, dir };
        for e in fs::read_dir(&store.dir)? {
            let p = e?.path();
            if p.extension().is_some_and(|s| s == "json") && legacy_name(&p) {
                store.import(&p, None);
            }
        }
        Ok(store)
    }
    pub fn import_workspace(&mut self, ws: &Path) {
        let dir = ws.join(".local_assistant");
        if fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir() && !crate::linked(&m)) {
            self.import(&dir.join("chat-session.json"), Some(ws));
        }
    }
    fn import(&mut self, path: &Path, ws: Option<&Path>) {
        if !path.is_file() {
            return;
        }
        if let Err(e) = self.import_one(path, ws) {
            eprintln!(
                "Legacy chat {}: {}",
                path.display(),
                crate::safe(&e.to_string())
            );
        }
    }
    fn import_one(&mut self, path: &Path, ws: Option<&Path>) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = fs::symlink_metadata(path)?;
        ensure!(
            before.is_file() && !crate::linked(&before),
            "linked files are skipped"
        );
        let raw = fs::read(path)?;
        let data: Value =
            serde_json::from_slice(raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&raw))?;
        let workspace = ws
            .or_else(|| data["workspace"].as_str().map(Path::new))
            .context("workspace missing")?;
        ensure!(workspace.is_absolute(), "invalid workspace");
        let msgs = data["messages"].as_array().context("messages missing")?;
        ensure!(msgs.iter().any(|m| m["role"] == "user"), "no user message");
        let mut hash = Sha256::new();
        hash.update(path.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update(&raw);
        let id = format!("{:x}", hash.finalize());
        let saved = data["saved_at"]
            .as_str()
            .and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())
            .and_then(|d| Local.from_local_datetime(&d).earliest())
            .and_then(|d| d.timestamp_nanos_opt())
            .unwrap_or(stamp(before.modified()?));
        tx.execute(
            "INSERT OR IGNORE INTO chats VALUES(?,?,?,?,1,?,?,?,?)",
            params![
                id,
                key(workspace),
                workspace.to_string_lossy(),
                saved,
                preview(msgs),
                msgs.len(),
                data["model"].as_str().unwrap_or(""),
                serde_json::to_string(msgs)?
            ],
        )?;
        if ws.is_some() || path.file_stem().is_some_and(|s| s.len() == 16) {
            tx.execute(
                "INSERT OR IGNORE INTO active VALUES(?,?)",
                params![key(workspace), id],
            )?;
        }
        tx.commit()?;
        if fs::symlink_metadata(path).is_ok_and(|m| {
            !crate::linked(&m)
                && m.len() == before.len()
                && m.modified().ok() == before.modified().ok()
        }) {
            fs::remove_file(path)?;
        }
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<Entry>> {
        let mut s=self.db.prepare("SELECT id,workspace,preview,saved_ns,message_count FROM chats ORDER BY saved_ns DESC,id")?;
        Ok(s.query_map([], |r| {
            Ok(Entry {
                id: r.get(0)?,
                workspace: PathBuf::from(r.get::<_, String>(1)?),
                preview: r.get(2)?,
                saved: r.get(3)?,
                count: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
    }
    pub fn current(&self, ws: &Path, latest: bool) -> Result<Option<String>> {
        let k = key(ws);
        let active: Option<Option<String>> = self
            .db
            .query_row(
                "SELECT chat_id FROM active WHERE workspace_key=?",
                [&k],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = active
            && (id.is_some() || !latest)
        {
            return Ok(id);
        }
        Ok(self
            .db
            .query_row(
                "SELECT id FROM chats WHERE workspace_key=? ORDER BY saved_ns DESC,id LIMIT 1",
                [k],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn load(&self, id: &str) -> Result<(PathBuf, Chat)> {
        let (ws, rev, body): (String, i64, String) = self
            .db
            .query_row(
                "SELECT workspace,revision,messages FROM chats WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .context("Saved chat is unavailable. Refresh /resume")?;
        let messages = crate::history::clean(serde_json::from_str(&body)?);
        ensure!(
            !messages.is_empty(),
            "The saved chat contains no usable messages"
        );
        Ok((
            ws.into(),
            Chat {
                id: Some(id.into()),
                revision: rev,
                messages,
                dirty: false,
            },
        ))
    }
    pub fn select(&self, ws: &Path, id: Option<&str>) -> Result<()> {
        let k = key(ws);
        if let Some(id) = id {
            ensure!(self.db.execute("INSERT OR REPLACE INTO active SELECT workspace_key,id FROM chats WHERE workspace_key=? AND id=?",params![k,id])?==1,"Chat no longer exists");
        } else {
            self.db
                .execute("INSERT OR REPLACE INTO active VALUES(?,NULL)", [k])?;
        }
        Ok(())
    }
    pub fn save(&mut self, ws: &Path, model: &str, chat: &mut Chat, explicit: bool) -> Result<()> {
        if !chat.dirty {
            return Ok(());
        }
        let body = serde_json::to_string(&chat.messages)?;
        let now = stamp(SystemTime::now());
        let k = key(ws);
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let updated = if let Some(id) = &chat.id {
            let n=tx.execute("UPDATE chats SET saved_ns=?,message_count=?,model=?,messages=?,revision=revision+1 WHERE workspace_key=? AND id=? AND revision=?",params![now,chat.messages.len(),model,body,k,id,chat.revision])?;
            if n == 0
                && !explicit
                && !tx.query_row("SELECT EXISTS(SELECT 1 FROM chats WHERE id=?)", [id], |r| {
                    r.get::<_, bool>(0)
                })?
            {
                bail!("Chat was deleted elsewhere. Use /save to keep a new copy.");
            }
            n == 1
        } else {
            false
        };
        let (id, revision) = if updated {
            (chat.id.clone().unwrap(), chat.revision + 1)
        } else {
            let id = Uuid::new_v4().simple().to_string();
            tx.execute(
                "INSERT INTO chats VALUES(?,?,?,?,1,?,?,?,?)",
                params![
                    id,
                    k,
                    ws.to_string_lossy(),
                    now,
                    preview(&chat.messages),
                    chat.messages.len(),
                    model,
                    body
                ],
            )?;
            (id, 1)
        };
        tx.execute("INSERT OR REPLACE INTO active VALUES(?,?)", params![k, id])?;
        tx.commit()?;
        if !updated && chat.id.is_some() {
            println!("Saved a separate copy of the changed chat.");
        }
        chat.id = Some(id);
        chat.revision = revision;
        chat.dirty = false;
        Ok(())
    }
    pub fn delete(&mut self, ws: &Path, all: bool) -> Result<usize> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prefix = format!(
            "{:x}",
            Sha256::digest(ws.to_string_lossy().to_lowercase().as_bytes())
        );
        for e in fs::read_dir(&self.dir)? {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "json" || x == "tmp")
                && legacy_name(&p)
                && (all
                    || p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .starts_with(&prefix[..16]))
            {
                fs::remove_file(p)?;
            }
        }
        let folder = ws.join(".local_assistant");
        let legacy = folder.join("chat-session.json");
        if legacy.is_file() && !crate::linked(&fs::symlink_metadata(&folder)?) {
            fs::remove_file(legacy)?;
        }
        let n = if all {
            tx.execute("DELETE FROM active", [])?;
            tx.execute("DELETE FROM chats", [])?
        } else {
            tx.execute("DELETE FROM active WHERE workspace_key=?", [key(ws)])?;
            tx.execute("DELETE FROM chats WHERE workspace_key=?", [key(ws)])?
        };
        tx.commit()?;
        Ok(n)
    }
}
