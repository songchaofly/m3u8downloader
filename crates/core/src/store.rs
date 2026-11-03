use crate::{AppError, Plan, Result, Settings, TaskInput, TaskView};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentRecord {
    pub track: usize,
    pub index: usize,
    pub identity: String,
    pub bytes: u64,
    pub digest: String,
    pub file: String,
}
#[derive(Serialize, Deserialize)]
struct PrivateTask {
    input: TaskInput,
    plan: Option<Plan>,
}
struct PendingSegment {
    task: String,
    record: SegmentRecord,
    done: std::sync::mpsc::Sender<Result<()>>,
}
pub struct Store {
    connection: Mutex<Connection>,
    pending_segments: Mutex<Vec<PendingSegment>>,
    vault: Option<Zeroizing<[u8; 32]>>,
    pub root: PathBuf,
}
pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        let path = root.join("tasks.sqlite");
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 1 {
            return Err(AppError::new(
                "MIGRATION_FAILED",
                "任务库来自更高版本，请使用兼容应用",
            ));
        }
        connection.execute_batch("CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY,view TEXT NOT NULL,secret BLOB);CREATE TABLE IF NOT EXISTS segments(task TEXT NOT NULL,track INTEGER NOT NULL,idx INTEGER NOT NULL,record TEXT NOT NULL,PRIMARY KEY(task,track,idx));CREATE TABLE IF NOT EXISTS attempts(id TEXT PRIMARY KEY,task TEXT NOT NULL,view TEXT NOT NULL);PRAGMA user_version=1;")?;
        let vault = load_vault().ok().map(Zeroizing::new);
        Ok(Self {
            connection: Mutex::new(connection),
            pending_segments: Mutex::new(Vec::new()),
            vault,
            root: root.into(),
        })
    }
    pub fn secure_available(&self) -> bool {
        self.vault.is_some()
    }
    pub fn save(
        &self,
        view: &TaskView,
        input: Option<&TaskInput>,
        plan: Option<&Plan>,
    ) -> Result<()> {
        let secret = if let (Some(input), Some(key)) = (input, self.vault.as_ref()) {
            let mut stored = input.clone();
            if !input.remember {
                stored.network.headers.clear();
                stored.network.origin_headers.clear();
                stored.manual_keys.clear();
                stored.manual_ivs.clear();
                stored.force_key = None;
                if let Ok(mut proxy) = url::Url::parse(&stored.network.proxy) {
                    let _ = proxy.set_username("");
                    let _ = proxy.set_password(None);
                    stored.network.proxy = proxy.to_string();
                }
            }
            let plain = Zeroizing::new(serde_json::to_vec(&PrivateTask {
                input: stored,
                plan: plan.cloned(),
            })?);
            let cipher = Aes256Gcm::new_from_slice(key.as_ref())
                .map_err(|_| AppError::new("STORE_FAILED", "安全存储初始化失败"))?;
            let mut nonce = [0u8; 12];
            rand::thread_rng().fill_bytes(&mut nonce);
            let mut blob = nonce.to_vec();
            blob.extend(
                cipher
                    .encrypt(Nonce::from_slice(&nonce), plain.as_slice())
                    .map_err(|_| AppError::new("STORE_FAILED", "无法保护任务数据"))?,
            );
            Some(blob)
        } else {
            None
        };
        let mut db = self.connection.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute("INSERT INTO tasks(id,view,secret) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET view=excluded.view,secret=COALESCE(excluded.secret,tasks.secret)",params![view.id,serde_json::to_string(view)?,secret])?;
        if !view.attempt_id.is_empty() {
            tx.execute(
                "INSERT OR REPLACE INTO attempts(id,task,view) VALUES(?1,?2,?3)",
                params![view.attempt_id, view.id, serde_json::to_string(view)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn load(&self) -> Result<Vec<(TaskView, Option<TaskInput>, Option<Plan>)>> {
        let db = self.connection.lock().unwrap();
        let mut stmt = db.prepare("SELECT view,secret FROM tasks ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (view, secret) = row?;
            let mut view: TaskView = serde_json::from_str(&view)?;
            let private = secret.and_then(|blob| {
                let key = self.vault.as_ref()?;
                if blob.len() < 13 {
                    return None;
                }
                let cipher = Aes256Gcm::new_from_slice(key.as_ref()).ok()?;
                let plain = Zeroizing::new(
                    cipher
                        .decrypt(Nonce::from_slice(&blob[..12]), &blob[12..])
                        .ok()?,
                );
                serde_json::from_slice::<PrivateTask>(&plain).ok()
            });
            if ["queued", "parsing", "downloading", "processing", "pausing"]
                .contains(&view.state.as_str())
            {
                view.state = "paused".into();
                view.stage = "上次执行已中断，可继续".into();
                view.speed = 0;
            }
            if private.is_none() {
                view.needs_credentials = true;
            }
            let (input, plan) = private
                .map(|p| (Some(p.input), p.plan))
                .unwrap_or((None, None));
            result.push((view, input, plan));
        }
        Ok(result)
    }
    pub fn segment(&self, task: &str, track: usize, index: usize) -> Result<Option<SegmentRecord>> {
        let db = self.connection.lock().unwrap();
        let mut stmt =
            db.prepare("SELECT record FROM segments WHERE task=?1 AND track=?2 AND idx=?3")?;
        let mut rows = stmt.query(params![task, track, index])?;
        if let Some(row) = rows.next()? {
            let s: String = row.get(0)?;
            Ok(Some(serde_json::from_str(&s)?))
        } else {
            Ok(None)
        }
    }
    pub fn commit_segment(&self, task: &str, record: &SegmentRecord) -> Result<()> {
        let (done, receipt) = std::sync::mpsc::channel();
        let leader = {
            let mut pending = self.pending_segments.lock().unwrap();
            let leader = pending.is_empty();
            pending.push(PendingSegment {
                task: task.into(),
                record: record.clone(),
                done,
            });
            leader
        };
        if leader {
            // Called by bounded blocking workers. Coalesce adjacent completions without
            // weakening FULL durability; no caller returns success before the commit.
            std::thread::sleep(std::time::Duration::from_millis(2));
            let batch = std::mem::take(&mut *self.pending_segments.lock().unwrap());
            let result = (|| -> Result<()> {
                let mut db = self.connection.lock().unwrap();
                let tx = db.transaction()?;
                for pending in &batch {
                    let record = &pending.record;
                    tx.execute("INSERT OR REPLACE INTO segments(task,track,idx,record) VALUES(?1,?2,?3,?4)",
                        params![pending.task, record.track, record.index, serde_json::to_string(record)?])?;
                }
                tx.commit()?;
                Ok(())
            })();
            for pending in batch {
                let _ = pending.done.send(result.clone());
            }
        }
        receipt
            .recv()
            .map_err(|_| AppError::new("STORE_FAILED", "分片事务没有返回提交结果"))?
    }
    pub fn remove(&self, id: &str) -> Result<()> {
        let mut db = self.connection.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute("DELETE FROM segments WHERE task=?1", [id])?;
        tx.execute("DELETE FROM attempts WHERE task=?1", [id])?;
        tx.execute("DELETE FROM tasks WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn clear_segments(&self, id: &str) -> Result<()> {
        self.connection
            .lock()
            .unwrap()
            .execute("DELETE FROM segments WHERE task=?1", [id])?;
        Ok(())
    }
    pub fn attempts(&self, id: &str) -> Result<Vec<TaskView>> {
        let db = self.connection.lock().unwrap();
        let mut statement =
            db.prepare("SELECT view FROM attempts WHERE task=?1 ORDER BY rowid DESC LIMIT 10")?;
        let values = statement.query_map([id], |row| row.get::<_, String>(0))?;
        let mut result = Vec::new();
        for value in values {
            result.push(serde_json::from_str(&value?)?);
        }
        Ok(result)
    }
    pub fn settings(&self) -> Result<Settings> {
        let p = self.root.join("settings.json");
        if p.exists() {
            let settings: Settings = serde_json::from_slice(&std::fs::read(p)?)?;
            if settings.schema_version != 1 {
                return Err(AppError::new("MIGRATION_FAILED", "配置文件来自不兼容版本"));
            }
            Ok(settings)
        } else {
            Ok(Settings::default())
        }
    }
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        let mut safe = settings.clone();
        safe.network.headers.clear();
        safe.network.origin_headers.clear();
        if let Ok(mut u) = url::Url::parse(&safe.network.proxy) {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            safe.network.proxy = u.to_string();
        }
        atomic_write(
            &self.root.join("settings.json"),
            &serde_json::to_vec_pretty(&safe)?,
        )?;
        Ok(())
    }
}
fn load_vault() -> Result<[u8; 32]> {
    let entry = keyring::Entry::new("io.m3u8studio.desktop", "task-vault-v1")
        .map_err(|_| AppError::new("CREDENTIAL_STORE_UNAVAILABLE", "操作系统安全存储不可用"))?;
    match entry.get_password() {
        Ok(s) => {
            let bytes = hex::decode(&s)
                .map_err(|_| AppError::new("CREDENTIAL_STORE_UNAVAILABLE", "安全存储密钥无效"))?;
            bytes
                .try_into()
                .map_err(|_| AppError::new("CREDENTIAL_STORE_UNAVAILABLE", "安全存储密钥长度无效"))
        }
        Err(keyring::Error::NoEntry) => {
            let mut key = [0; 32];
            rand::thread_rng().fill_bytes(&mut key);
            entry.set_password(&hex::encode(key)).map_err(|_| {
                AppError::new("CREDENTIAL_STORE_UNAVAILABLE", "无法写入系统安全存储")
            })?;
            Ok(key)
        }
        Err(_) => Err(AppError::new(
            "CREDENTIAL_STORE_UNAVAILABLE",
            "操作系统安全存储已锁定或不可用",
        )),
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::filesystem::atomic_write(path, bytes)
}
