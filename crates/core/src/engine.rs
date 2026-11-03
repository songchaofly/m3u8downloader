use crate::{
    crypto, media,
    network::Network,
    plan,
    store::{hash, SegmentRecord, Store},
    AppError, Plan, Playlist, Result, Segment, Settings, TaskInput, TaskView,
};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, Semaphore};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

struct Task {
    view: TaskView,
    input: Option<TaskInput>,
    plan: Option<Plan>,
    cancel: CancellationToken,
    running: bool,
    credentials_available: bool,
    stop_state: String,
    last_event: Instant,
}
pub struct Engine {
    pub store: Store,
    tasks: Mutex<BTreeMap<String, Arc<Mutex<Task>>>>,
    settings: RwLock<Settings>,
    global: Arc<Semaphore>,
    slots: Arc<Semaphore>,
    processing: Arc<Semaphore>,
    events: broadcast::Sender<TaskView>,
    previews: Mutex<HashMap<String, PreviewSession>>,
    local_previews: Mutex<HashMap<String, (CancellationToken, tempfile::TempDir)>>,
    _lock: std::fs::File,
}
struct PreviewSession {
    net: Network,
    cancel: CancellationToken,
    resources: HashMap<String, PreviewResource>,
    input: TaskInput,
    created: Instant,
    keys: Arc<HashMap<String, Zeroizing<[u8; 16]>>>,
}
#[derive(Clone)]
struct PreviewResource {
    uri: String,
    range: Option<crate::ByteRange>,
    key: Option<crate::KeyContext>,
    sequence: u64,
    kind: String,
    map: Option<String>,
}
#[derive(Serialize, Deserialize)]
pub struct PreviewInfo {
    pub id: String,
    pub manifest: String,
}
#[derive(Serialize)]
pub struct Diagnostics {
    pub secure_storage: bool,
    pub data_dir: String,
    pub cache_dir: String,
    pub capabilities: Option<media::Capabilities>,
}

impl Engine {
    pub fn open(root: &Path) -> Result<Arc<Self>> {
        std::fs::create_dir_all(root)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("instance.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .map_err(|_| AppError::new("INSTANCE_RUNNING", "此数据目录已由另一个应用实例使用"))?;
        let store = Store::open(root)?;
        let mut settings = store.settings()?;
        if settings.output_dir.is_empty() {
            settings.output_dir = root.join("downloads").to_string_lossy().into_owned();
        }
        if settings.cache_dir.is_empty() {
            settings.cache_dir = root.join("cache").to_string_lossy().into_owned();
        }
        if let Ok(executable) = std::env::current_exe() {
            if let Some(parent) = executable.parent() {
                for (configured, name) in [
                    (&mut settings.ffmpeg, "ffmpeg"),
                    (&mut settings.ffprobe, "ffprobe"),
                ] {
                    let binary = parent
                        .join("runtime")
                        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
                    let normalized = configured.replace('\\', "/");
                    if binary.is_file()
                        && (configured == name
                            || normalized.ends_with(&format!("runtime/{name}.exe")))
                    {
                        *configured = binary.to_string_lossy().into_owned();
                    }
                }
            }
        }
        std::fs::create_dir_all(&settings.cache_dir)?;
        std::fs::create_dir_all(&settings.output_dir)?;
        let tasks = store
            .load()?
            .into_iter()
            .map(|(view, input, plan)| {
                let id = view.id.clone();
                let credentials_available = !view.needs_credentials;
                (
                    id,
                    Arc::new(Mutex::new(Task {
                        view,
                        input,
                        plan,
                        cancel: CancellationToken::new(),
                        running: false,
                        credentials_available,
                        stop_state: "paused".into(),
                        last_event: Instant::now(),
                    })),
                )
            })
            .collect();
        let (events, _) = broadcast::channel(256);
        let engine = Arc::new(Self {
            global: Arc::new(Semaphore::new(settings.max_requests.clamp(1, 64))),
            slots: Arc::new(Semaphore::new(settings.max_tasks.clamp(1, 8))),
            processing: Arc::new(Semaphore::new(settings.max_processing.clamp(1, 4))),
            settings: RwLock::new(settings),
            store,
            tasks: Mutex::new(tasks),
            events,
            previews: Mutex::new(HashMap::new()),
            local_previews: Mutex::new(HashMap::new()),
            _lock: lock,
        });
        if engine.settings().history_days > 0 {
            engine.prune_history(engine.settings().history_days)?;
        }
        Ok(engine)
    }
    fn task(&self, id: &str) -> Result<Arc<Mutex<Task>>> {
        self.tasks
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::new("TASK_NOT_FOUND", "任务不存在"))
    }
    pub fn subscribe(&self) -> broadcast::Receiver<TaskView> {
        self.events.subscribe()
    }
    pub fn list(&self) -> Vec<TaskView> {
        let mut list = self
            .tasks
            .lock()
            .unwrap()
            .values()
            .map(|t| t.lock().unwrap().view.clone())
            .collect::<Vec<_>>();
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        list
    }
    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }
    pub fn save_settings(&self, new: Settings) -> Result<()> {
        if !(1..=8).contains(&new.max_tasks)
            || !(1..=64).contains(&new.max_requests)
            || !(1..=4).contains(&new.max_processing)
            || !["DEBUG", "INFO", "WARN", "ERROR"].contains(&new.log_level.as_str())
            || new.schema_version != 1
            || new.history_days > 36500
        {
            return Err(AppError::new("INPUT_INVALID", "并发配置超出范围"));
        }
        Network::new(
            new.network.clone(),
            "https://configuration.invalid",
            None,
            self.global.clone(),
        )?;
        let tasks = self.list();
        if tasks.iter().any(|t| {
            ["queued", "parsing", "downloading", "processing", "pausing"]
                .contains(&t.state.as_str())
        }) {
            return Err(AppError::new("TASK_BUSY", "请先暂停全部任务再更改设置"));
        }
        std::fs::create_dir_all(&new.output_dir)?;
        std::fs::create_dir_all(&new.cache_dir)?;
        let mut settings = self.settings.write().unwrap();
        let old = settings.clone();
        if new.cache_dir != old.cache_dir
            && tasks.iter().any(|t| {
                !["completed", "completed_with_gaps", "cancelled"].contains(&t.state.as_str())
            })
        {
            return Err(AppError::new(
                "TASK_BUSY",
                "未完成任务仍引用原缓存目录，请完成或删除这些任务后再修改",
            ));
        }
        let changes = [
            (&self.global, old.max_requests, new.max_requests),
            (&self.slots, old.max_tasks, new.max_tasks),
            (&self.processing, old.max_processing, new.max_processing),
        ];
        let mut reserved = Vec::new();
        for (semaphore, old, new) in changes {
            if old != new {
                let permit = semaphore
                    .clone()
                    .try_acquire_many_owned(old as u32)
                    .map_err(|_| {
                        AppError::new(
                            "TASK_BUSY",
                            "请先停止使用当前并发额度的任务、解析或预览，再修改并发设置",
                        )
                    })?;
                reserved.push((semaphore, permit, new));
            }
        }
        self.store.save_settings(&new)?;
        for (semaphore, permit, count) in reserved {
            permit.forget();
            semaphore.add_permits(count);
        }
        *settings = new;
        Ok(())
    }
    pub fn duplicate_sources(&self, sources: &[String]) -> Vec<String> {
        let tasks = self.tasks.lock().unwrap();
        let mut seen = tasks
            .values()
            .filter_map(|t| t.lock().unwrap().input.as_ref().map(|i| i.source.clone()))
            .collect::<std::collections::HashSet<_>>();
        sources
            .iter()
            .filter(|source| !seen.insert((*source).clone()))
            .cloned()
            .collect()
    }
    pub async fn test_connection(&self, input: &TaskInput) -> Result<serde_json::Value> {
        let mut config = input.network.clone();
        config.retries = 0;
        config.request_timeout = 20;
        let net = Network::new(
            config,
            &input.source,
            input.local_root.as_deref(),
            self.global.clone(),
        )?;
        let start = Instant::now();
        let source = crate::playlist::source_url(&input.source)?;
        let resource = net
            .get(
                source.as_str(),
                None,
                10 * 1024 * 1024,
                &CancellationToken::new(),
            )
            .await?;
        Ok(
            serde_json::json!({"elapsed_ms":start.elapsed().as_millis(),"bytes":resource.bytes.len(),"target":crate::network::display_url(&resource.final_url)}),
        )
    }
    pub fn task_details(&self, id: &str, offset: usize, limit: usize) -> Result<serde_json::Value> {
        let task = self.task(id)?;
        let task = task.lock().unwrap();
        let mut segments = Vec::new();
        let mut total = 0;
        if let Some(plan) = &task.plan {
            for (ti, track) in plan.tracks.iter().enumerate() {
                for segment in &track.playlist.segments {
                    let pos = total;
                    total += 1;
                    if pos < offset || segments.len() >= limit.min(100) {
                        continue;
                    }
                    let resource = format!("{ti}:{}", segment.index);
                    let record = self.store.segment(id, ti, segment.index)?;
                    let state = if task.view.missing.contains(&resource) {
                        "skipped"
                    } else if task
                        .view
                        .error
                        .as_ref()
                        .and_then(|e| e.resource_id.as_ref())
                        == Some(&resource)
                    {
                        "failed"
                    } else if record.is_some() {
                        "available"
                    } else {
                        "pending"
                    };
                    segments.push(serde_json::json!({"resource":resource,"track":track.kind,"sequence":segment.sequence,"duration":segment.duration,"state":state,"bytes":record.map(|r|r.bytes).unwrap_or(0),"source":crate::network::display_url(&segment.uri),"context":segment.key.as_ref().map(|k|format!("{ti}:{}",k.id)),"iv":segment.key.as_ref().map(|k|if k.iv.is_some(){"explicit"}else{"sequence"}),"discontinuity":segment.discontinuity,"range":segment.range}));
                }
            }
        }
        Ok(
            serde_json::json!({"total":total,"segments":segments,"attempts":self.store.attempts(id)?}),
        )
    }
    pub fn prune_history(&self, days: u64) -> Result<usize> {
        if days > 36500 {
            return Err(AppError::new("INPUT_INVALID", "历史保留期超出范围"));
        }
        let mut removed = 0;
        for view in self.list() {
            if !["completed", "completed_with_gaps", "cancelled"].contains(&view.state.as_str()) {
                continue;
            }
            let age = chrono::DateTime::parse_from_rfc3339(&view.created_at)
                .map(|v| (chrono::Utc::now() - v.with_timezone(&chrono::Utc)).num_days())
                .unwrap_or(0);
            if days == 0 || age >= days as i64 {
                self.clear_cache(&view.id)?;
                self.store.remove(&view.id)?;
                self.tasks.lock().unwrap().remove(&view.id);
                removed += 1;
            }
        }
        Ok(removed)
    }
    pub async fn inspect(&self, input: &TaskInput) -> Result<Playlist> {
        let n = Network::new(
            input.network.clone(),
            &input.source,
            input.local_root.as_deref(),
            self.global.clone(),
        )?;
        plan::fetch_playlist(
            &n,
            &input.source,
            input.base_url.as_deref(),
            &CancellationToken::new(),
        )
        .await
    }
    pub fn add(&self, mut input: TaskInput) -> Result<TaskView> {
        let settings = self.settings();
        if input.output_dir.is_empty() {
            input.output_dir = settings.output_dir;
        }
        media::validate_input(&input)?;
        Network::new(
            input.network.clone(),
            &input.source,
            input.local_root.as_deref(),
            self.global.clone(),
        )?;
        if input.remember && !self.store.secure_available() {
            return Err(AppError::new(
                "CREDENTIAL_STORE_UNAVAILABLE",
                "系统安全存储不可用，不能记住凭证",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let title = if input.title.trim().is_empty() {
            format!("视频 {}", &id[..8])
        } else {
            media::safe_name(&input.title)
        };
        input.title = title.clone();
        let view = TaskView {
            id: id.clone(),
            title,
            source_display: crate::network::display_url(&input.source),
            state: "ready".into(),
            stage: "待下载".into(),
            done: 0,
            total: 0,
            bytes: 0,
            speed: 0,
            progress: 0.0,
            resolution: "unknown".into(),
            output: None,
            error: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            attempt_id: String::new(),
            event_seq: 0,
            missing: Vec::new(),
            needs_credentials: needs_credentials(&input) || !self.store.secure_available(),
            processing_ready: false,
        };
        self.store.save(&view, Some(&input), None)?;
        self.tasks.lock().unwrap().insert(
            id,
            Arc::new(Mutex::new(Task {
                view: view.clone(),
                input: Some(input),
                plan: None,
                cancel: CancellationToken::new(),
                running: false,
                credentials_available: true,
                stop_state: "paused".into(),
                last_event: Instant::now(),
            })),
        );
        let _ = self.events.send(view.clone());
        Ok(view)
    }
    pub fn input(&self, id: &str) -> Result<Option<TaskInput>> {
        Ok(self.task(id)?.lock().unwrap().input.clone())
    }
    pub fn update_input(&self, id: &str, input: TaskInput) -> Result<()> {
        self.update_options(id, input, true)
    }
    pub fn update_options(
        &self,
        id: &str,
        input: TaskInput,
        credentials_supplied: bool,
    ) -> Result<()> {
        media::validate_input(&input)?;
        if input.remember && !self.store.secure_available() {
            return Err(AppError::new(
                "CREDENTIAL_STORE_UNAVAILABLE",
                "系统安全存储不可用，不能记住凭证",
            ));
        }
        Network::new(
            input.network.clone(),
            &input.source,
            input.local_root.as_deref(),
            self.global.clone(),
        )?;
        let task = self.task(id)?;
        let mut t = task.lock().unwrap();
        if t.running {
            return Err(AppError::new("TASK_BUSY", "请先暂停任务"));
        }
        if ["completed", "completed_with_gaps"].contains(&t.view.state.as_str()) {
            return Err(AppError::new("TASK_TERMINAL", "请复制为新任务后修改"));
        }
        let identity_changed = t.input.as_ref().is_some_and(|old| {
            old.source != input.source
                || old.variant != input.variant
                || old.audio != input.audio
                || old.subtitle != input.subtitle
                || old.video_only != input.video_only
        });
        if identity_changed {
            let only_remove_subtitle = t.input.as_ref().is_some_and(|old| {
                old.source == input.source
                    && old.variant == input.variant
                    && old.audio == input.audio
                    && old.video_only == input.video_only
                    && old.subtitle.is_some()
                    && input.subtitle.is_none()
            });
            if only_remove_subtitle && t.plan.is_some() {
                t.plan
                    .as_mut()
                    .unwrap()
                    .tracks
                    .retain(|track| track.kind != "subtitle");
                t.view.processing_ready = true;
            } else {
                t.plan = None;
                t.view.processing_ready = false;
            }
        }
        t.view.title = media::safe_name(&input.title);
        t.view.source_display = crate::network::display_url(&input.source);
        t.credentials_available |= credentials_supplied;
        t.view.needs_credentials =
            !t.credentials_available || needs_credentials(&input) || !self.store.secure_available();
        t.input = Some(input);
        self.store
            .save(&t.view, t.input.as_ref(), t.plan.as_ref())?;
        Ok(())
    }
    pub fn start(self: &Arc<Self>, id: &str) -> Result<()> {
        let task = self.task(id)?;
        {
            let mut t = task.lock().unwrap();
            if t.running {
                return Ok(());
            }
            if ["completed", "completed_with_gaps"].contains(&t.view.state.as_str()) {
                return Err(AppError::new("TASK_TERMINAL", "已完成任务请复制后重新下载"));
            }
            if t.input.is_none() {
                return Err(AppError::new(
                    "CREDENTIAL_REQUIRED",
                    "请重新提供来源与请求配置",
                ));
            }
            if !t.credentials_available && !t.view.processing_ready {
                return Err(AppError::new(
                    "CREDENTIAL_REQUIRED",
                    "凭证未跨重启保存，请编辑任务补充配置",
                ));
            }
            t.cancel = CancellationToken::new();
            t.running = true;
            t.stop_state = "paused".into();
            t.view.state = "queued".into();
            t.view.stage = "等待下载名额".into();
            t.view.attempt_id = uuid::Uuid::new_v4().to_string();
            t.view.event_seq += 1;
            t.view.error = None;
            self.store
                .save(&t.view, t.input.as_ref(), t.plan.as_ref())?;
            let _ = self.events.send(t.view.clone());
        }
        let engine = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let result = engine.run(&id).await;
            engine.finish_run(&id, result);
        });
        Ok(())
    }
    pub async fn stop(&self, id: &str, cancelled: bool) -> Result<()> {
        let task = self.task(id)?;
        {
            let mut t = task.lock().unwrap();
            if ["completed", "completed_with_gaps"].contains(&t.view.state.as_str()) {
                return Ok(());
            }
            t.stop_state = if cancelled { "cancelled" } else { "paused" }.into();
            t.cancel.cancel();
            if !t.running {
                t.view.state = t.stop_state.clone();
                t.view.stage = if cancelled { "已取消" } else { "已暂停" }.into();
                self.store
                    .save(&t.view, t.input.as_ref(), t.plan.as_ref())?;
                let _ = self.events.send(t.view.clone());
                return Ok(());
            }
            t.view.state = "pausing".into();
            let _ = self.events.send(t.view.clone());
        }
        for _ in 0..150 {
            if !task.lock().unwrap().running {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Err(AppError::new("TASK_STOPPING", "任务正在停止，请稍后重试"))
    }
    pub async fn shutdown(&self) {
        let ids = self.list().into_iter().map(|t| t.id).collect::<Vec<_>>();
        for id in ids {
            let _ = self.stop(&id, false).await;
        }
        let previews = self
            .previews
            .lock()
            .unwrap()
            .drain()
            .map(|(_, p)| p)
            .collect::<Vec<_>>();
        for p in previews {
            p.cancel.cancel();
        }
        for (_, (cancel, _)) in self.local_previews.lock().unwrap().drain() {
            cancel.cancel();
        }
    }
    pub async fn delete(&self, id: &str, cache: bool) -> Result<()> {
        self.stop(id, true).await?;
        if cache {
            self.clear_cache(id)?;
        }
        self.store.remove(id)?;
        self.tasks.lock().unwrap().remove(id);
        Ok(())
    }
    pub fn clear_cache(&self, id: &str) -> Result<()> {
        uuid::Uuid::parse_str(id).map_err(|_| AppError::new("INPUT_INVALID", "任务 ID 无效"))?;
        if self.task(id)?.lock().unwrap().running {
            return Err(AppError::new("TASK_BUSY", "活动任务的缓存不能清理"));
        }
        let root = std::fs::canonicalize(&self.settings().cache_dir)?;
        let path = root.join(id);
        if path.exists() {
            safe_remove_tree(&root, &path)?;
        }
        self.store.clear_segments(id)?;
        Ok(())
    }
    fn update(&self, id: &str, persist: bool, f: impl FnOnce(&mut TaskView)) -> Result<()> {
        let task = self.task(id)?;
        let mut t = task.lock().unwrap();
        f(&mut t.view);
        t.view.event_seq += 1;
        if persist {
            self.store
                .save(&t.view, t.input.as_ref(), t.plan.as_ref())?;
            self.log(&t.view);
        }
        if persist || t.last_event.elapsed() >= Duration::from_millis(250) {
            let _ = self.events.send(t.view.clone());
            t.last_event = Instant::now();
        }
        Ok(())
    }
    fn finish_run(&self, id: &str, result: Result<()>) {
        let Ok(task) = self.task(id) else {
            return;
        };
        let mut t = task.lock().unwrap();
        t.running = false;
        t.view.speed = 0;
        if let Err(error) = result {
            if error.code == "CANCELLED" {
                t.view.state = t.stop_state.clone();
                t.view.stage = if t.stop_state == "cancelled" {
                    "已取消"
                } else {
                    "已暂停"
                }
                .into();
            } else {
                t.view.state = if [
                    "DISK_FULL",
                    "CREDENTIAL_REQUIRED",
                    "HTTP_AUTH",
                    "SOURCE_CHANGED",
                    "MISSING_CONFIRMATION",
                ]
                .contains(&error.code.as_str())
                {
                    "waiting"
                } else {
                    "failed"
                }
                .into();
                t.view.stage = error.message.clone();
                t.view.error = Some(error);
            }
        }
        t.view.event_seq += 1;
        if let Err(e) = self.store.save(&t.view, t.input.as_ref(), t.plan.as_ref()) {
            t.view.state = "failed".into();
            t.view.error = Some(e);
        }
        self.log(&t.view);
        let _ = self.events.send(t.view.clone());
    }
    async fn run(self: &Arc<Self>, id: &str) -> Result<()> {
        let task = self.task(id)?;
        let (
            input,
            cancel,
            old_plan,
            created,
            processing_ready,
            old_missing,
            credentials_available,
        ) = {
            let t = task.lock().unwrap();
            (
                t.input.clone().unwrap(),
                t.cancel.clone(),
                t.plan.clone(),
                t.view.created_at.clone(),
                t.view.processing_ready,
                t.view.missing.clone(),
                t.credentials_available,
            )
        };
        let slot = tokio::select! {_=cancel.cancelled()=>return Err(AppError::cancelled()),s=self.slots.acquire()=>s.map_err(|_|AppError::cancelled())?};
        self.update(id, true, |v| {
            v.state = "parsing".into();
            v.stage = "解析清单与轨道".into();
            v.done = 0;
            v.missing.clear();
        })?;
        let net = Network::new(
            input.network.clone(),
            &input.source,
            input.local_root.as_deref(),
            self.global.clone(),
        )?;
        tokio::fs::create_dir_all(&input.output_dir).await?;
        let cache = crate::filesystem::task_cache(Path::new(&self.settings().cache_dir), id)?;
        let offline = if processing_ready {
            if let Some(old) = &old_plan {
                self.cache_complete(
                    id,
                    old,
                    &input,
                    &cache,
                    &old_missing,
                    &cancel,
                    !credentials_available,
                )
                .await?
            } else {
                false
            }
        } else {
            false
        };
        if !offline && !credentials_available {
            return Err(AppError::new(
                "CREDENTIAL_REQUIRED",
                "剩余资源需要未保存的凭证，请补充请求配置或密钥后继续",
            ));
        }
        let plan = if offline {
            old_plan.clone().unwrap()
        } else {
            plan::build(&net, &input, &cancel).await?
        };
        if let Some(old) = old_plan {
            if plan_identity(&old)? != plan_identity(&plan)? {
                return Err(AppError::new(
                    "SOURCE_CHANGED",
                    "播放列表内容已变化，请核对来源并创建新任务，避免混用媒体",
                ));
            }
        }
        {
            task.lock().unwrap().plan = Some(plan.clone());
        }
        let total = plan.tracks.iter().map(|t| t.playlist.segments.len()).sum();
        self.update(id, true, |v| {
            v.state = "downloading".into();
            v.stage = "下载与校验分片".into();
            v.total = total;
            v.bytes = 0;
            v.progress = 0.0;
            v.resolution = plan.resolution.clone();
            v.processing_ready = offline;
            if offline {
                v.missing = old_missing.clone();
                v.done = total.saturating_sub(old_missing.len());
            }
        })?;
        let mut keys = HashMap::<String, Zeroizing<[u8; 16]>>::new();
        for (ti, track) in plan.tracks.iter().enumerate() {
            for segment in &track.playlist.segments {
                if offline
                    || self
                        .cached_bytes(id, ti, &track.kind, segment, &input, &cache)
                        .await?
                        .is_some()
                {
                    continue;
                }
                if let Some(k) = &segment.key {
                    let context = format!("{ti}:{}", k.id);
                    if !keys.contains_key(&context) {
                        let manual = input
                            .manual_keys
                            .get(&context)
                            .or_else(|| input.manual_keys.get(&k.id.to_string()));
                        let key = if let Some(v) = manual {
                            crypto::decode_key(v)?
                        } else {
                            net.get(&k.uri, None, 16, &cancel)
                                .await?
                                .bytes
                                .try_into()
                                .map_err(|_| {
                                    AppError::new("KEY_INVALID", "密钥响应必须恰好为 16 字节")
                                })?
                        };
                        keys.insert(context, Zeroizing::new(key));
                    }
                }
            }
        }
        let keys = Arc::new(keys);
        let input = Arc::new(input);
        let jobs = plan
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(ti, t)| {
                t.playlist
                    .segments
                    .iter()
                    .cloned()
                    .map(move |s| (ti, t.kind.clone(), s))
            })
            .collect::<Vec<_>>();
        let jobs = jobs.into_iter().filter(|_| !offline).collect::<Vec<_>>();
        let cache_ref = &cache;
        let net_ref = &net;
        let input_ref = &input;
        let keys_ref = &keys;
        let cancel_ref = &cancel;
        let mut work = stream::iter(jobs.into_iter().map(|(ti, kind, segment)| async move {
            let result = self
                .segment(
                    id, ti, &kind, &segment, cache_ref, net_ref, input_ref, keys_ref, cancel_ref,
                )
                .await;
            let result = result.map_err(|mut e| {
                e.resource_id = Some(format!("{ti}:{}", segment.index));
                e.stage = Some(
                    if e.code.starts_with("DECRYPT") || e.code.starts_with("KEY") {
                        "decrypt"
                    } else {
                        "download"
                    }
                    .into(),
                );
                e
            });
            (ti, segment.index, kind, result)
        }))
        .buffer_unordered(input.network.concurrency);
        let mut samples = std::collections::VecDeque::new();
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        loop {
            let item = tokio::select! {
                _ = cancel.cancelled() => {
                    while work.next().await.is_some() {}
                    return Err(AppError::cancelled());
                },
                _ = ticker.tick() => {
                    let now = Instant::now();
                    let received = net.received.load(std::sync::atomic::Ordering::Relaxed);
                    samples.push_back((now, received));
                    while samples.len() > 1 && samples[0].0.elapsed() > Duration::from_secs(3) { samples.pop_front(); }
                    let speed = samples.front().map(|(time, bytes)| ((received - bytes) as f64 / time.elapsed().as_secs_f64().max(0.25)) as u64).unwrap_or(0);
                    self.update(id, false, |v| v.speed = speed)?;
                    continue;
                },
                item = work.next() => item
            };
            let Some((ti, index, kind, result)) = item else {
                break;
            };
            if offline && old_missing.contains(&format!("{ti}:{index}")) {
                continue;
            }
            match result {
                Ok(bytes) => self.update(id, false, |v| {
                    v.done += 1;
                    v.bytes += bytes;
                    v.progress = v.done as f64 / v.total.max(1) as f64;
                })?,
                Err(e)
                    if input.allow_gaps
                        && kind != "subtitle"
                        && [
                            "HTTP_NOT_FOUND",
                            "NETWORK_TIMEOUT",
                            "NETWORK_FAILED",
                            "HTTP_ERROR",
                        ]
                        .contains(&e.code.as_str()) =>
                {
                    self.update(id, false, |v| v.missing.push(format!("{ti}:{index}")))?
                }
                Err(e) => {
                    cancel.cancel();
                    while work.next().await.is_some() {}
                    return Err(e);
                }
            }
        }
        drop(work);
        drop(slot);
        let _process = tokio::select! {_=cancel.cancelled()=>return Err(AppError::cancelled()),s=self.processing.acquire()=>s.map_err(|_|AppError::cancelled())?};
        let missing = task.lock().unwrap().view.missing.clone();
        self.update(id, true, |v| {
            v.state = "processing".into();
            v.stage = "合并、转封装与验证".into();
            v.progress = 0.0;
            v.speed = 0;
            v.processing_ready = true;
        })?;
        if !missing.is_empty() && !input.confirm_gaps {
            return Err(AppError::new(
                "MISSING_CONFIRMATION",
                "请查看缺失分片范围，明确确认后生成不完整输出；也可关闭跳过选项后重试",
            ));
        }
        let prepared = media::process(
            &input,
            &plan,
            &cache,
            id,
            &created,
            &missing,
            &cancel,
            |p| {
                let _ = self.update(id, false, |v| v.progress = p);
            },
        )
        .await?;
        {
            let mut task = task.lock().unwrap();
            let output = prepared.publish(&cancel)?;
            task.view.output = Some(output.path);
            task.view.state = if missing.is_empty() {
                "completed"
            } else {
                "completed_with_gaps"
            }
            .into();
            task.view.stage = if missing.is_empty() {
                "已完成"
            } else {
                "已完成（有缺失）"
            }
            .into();
            task.view.progress = 1.0;
            task.view.bytes = output.bytes;
        }
        if !input.keep_cache && missing.is_empty() {
            let root = std::fs::canonicalize(&self.settings().cache_dir)?;
            if safe_remove_tree(&root, &cache).is_ok() {
                let _ = self.store.clear_segments(id);
            }
        }
        Ok(())
    }
    async fn cached_bytes(
        &self,
        id: &str,
        ti: usize,
        kind: &str,
        segment: &Segment,
        input: &TaskInput,
        cache: &Path,
    ) -> Result<Option<u64>> {
        let file = cache.join(format!(
            "{ti}-{}.{}",
            segment.index,
            if kind == "subtitle" { "vtt" } else { "ts" }
        ));
        if let Some(record) = self.store.segment(id, ti, segment.index)? {
            if record.identity == segment_identity(ti, segment, input)? && file.exists() {
                let bytes = tokio::fs::read(file).await?;
                if bytes.len() as u64 == record.bytes && hash(&bytes) == record.digest {
                    return Ok(Some(record.bytes));
                }
            }
        }
        Ok(None)
    }
    async fn cache_complete(
        &self,
        id: &str,
        plan: &Plan,
        input: &TaskInput,
        cache: &Path,
        missing: &[String],
        cancel: &CancellationToken,
        trust_saved_config: bool,
    ) -> Result<bool> {
        for (ti, track) in plan.tracks.iter().enumerate() {
            for segment in &track.playlist.segments {
                if cancel.is_cancelled() {
                    return Err(AppError::cancelled());
                }
                if missing.contains(&format!("{ti}:{}", segment.index))
                    && input.allow_gaps
                    && track.kind != "subtitle"
                {
                    continue;
                }
                if trust_saved_config {
                    let Some(record) = self.store.segment(id, ti, segment.index)? else {
                        return Ok(false);
                    };
                    let path = cache.join(format!(
                        "{ti}-{}.{}",
                        segment.index,
                        if track.kind == "subtitle" {
                            "vtt"
                        } else {
                            "ts"
                        }
                    ));
                    let Ok(bytes) = tokio::fs::read(path).await else {
                        return Ok(false);
                    };
                    if bytes.len() as u64 != record.bytes || hash(&bytes) != record.digest {
                        return Ok(false);
                    }
                } else if self
                    .cached_bytes(id, ti, &track.kind, segment, input, cache)
                    .await?
                    .is_none()
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    async fn segment(
        self: &Arc<Self>,
        id: &str,
        ti: usize,
        kind: &str,
        segment: &Segment,
        cache: &Path,
        net: &Network,
        input: &TaskInput,
        keys: &HashMap<String, Zeroizing<[u8; 16]>>,
        cancel: &CancellationToken,
    ) -> Result<u64> {
        if cancel.is_cancelled() {
            return Err(AppError::cancelled());
        }
        let identity = segment_identity(ti, segment, input)?;
        let file = cache.join(format!(
            "{ti}-{}.{}",
            segment.index,
            if kind == "subtitle" { "vtt" } else { "ts" }
        ));
        if let Some(bytes) = self
            .cached_bytes(id, ti, kind, segment, input, cache)
            .await?
        {
            return Ok(bytes);
        }
        let settings = self.settings();
        if fs2::available_space(cache)? < settings.low_disk_mib.saturating_mul(1024 * 1024)
            || fs2::available_space(Path::new(&input.output_dir))?
                < settings.low_disk_mib.saturating_mul(1024 * 1024)
        {
            return Err(AppError::new(
                "DISK_FULL",
                "缓存或输出磁盘剩余空间不足，请释放空间后继续",
            ));
        }
        let mut bytes = net
            .get(
                &segment.uri,
                segment.range.as_ref(),
                64 * 1024 * 1024,
                cancel,
            )
            .await?
            .bytes;
        if let Some(k) = &segment.key {
            let context = format!("{ti}:{}", k.id);
            let key = keys
                .get(&context)
                .ok_or_else(|| AppError::new("KEY_INVALID", "密钥上下文未就绪"))?;
            let manual = input
                .manual_ivs
                .get(&context)
                .or_else(|| input.manual_ivs.get(&k.id.to_string()))
                .map(String::as_str);
            bytes = crypto::decrypt(
                &bytes,
                key,
                &crypto::iv(manual.or(k.iv.as_deref()), segment.sequence)?,
            )?;
        } else if let Some(k) = forced_key(input, ti, segment.sequence) {
            bytes = crypto::decrypt(
                &bytes,
                &crypto::decode_key(k)?,
                &crypto::iv(
                    input.manual_ivs.get("manual").map(String::as_str),
                    segment.sequence,
                )?,
            )?;
        }
        if kind == "subtitle" {
            if let Some(map) = &segment.map {
                let header = net.get(map, None, 1024 * 1024, cancel).await?.bytes;
                let mut combined = header;
                combined.extend_from_slice(b"\n\n");
                combined.extend(bytes);
                bytes = combined;
            }
        }
        let length = bytes.len() as u64;
        let engine = self.clone();
        let task_id = id.to_owned();
        let index = segment.index;
        let subtitle = kind == "subtitle";
        let cancel = cancel.clone();
        // File flushes and SQLite commits must not serialize polling all network futures.
        // The run loop drains these jobs on stop/error before reporting a terminal state.
        tokio::task::spawn_blocking(move || -> Result<()> {
            if cancel.is_cancelled() {
                return Err(AppError::cancelled());
            }
            crypto::validate_media(&bytes, subtitle)?;
            let digest = hash(&bytes);
            crate::filesystem::atomic_write(&file, &bytes)?;
            if cancel.is_cancelled() {
                return Err(AppError::cancelled());
            }
            engine.store.commit_segment(
                &task_id,
                &SegmentRecord {
                    track: ti,
                    index,
                    identity,
                    bytes: length,
                    digest,
                    file: file.file_name().unwrap().to_string_lossy().into_owned(),
                },
            )
        })
        .await
        .map_err(|_| AppError::new("STORE_FAILED", "分片提交工作异常退出"))??;
        if self.settings().log_level == "DEBUG" {
            use std::io::Write;
            let view = self.task(id)?.lock().unwrap().view.clone();
            self.log(&view);
            let value = serde_json::json!({"time":chrono::Utc::now().to_rfc3339(),"level":"DEBUG","task_id":id,"attempt_id":view.attempt_id,"resource_id":format!("{ti}:{}",segment.index),"stage":"segment_committed","bytes":length});
            if let Ok(mut log) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.store.root.join("logs/application.jsonl"))
            {
                let _ = writeln!(log, "{value}");
            }
        }
        Ok(length)
    }
    fn log(&self, view: &TaskView) {
        use std::io::Write;
        let dir = self.store.root.join("logs");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("application.jsonl");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 10 * 1024 * 1024) {
            let rotated = dir.join(format!("{}.jsonl", chrono::Utc::now().timestamp_millis()));
            let _ = std::fs::rename(&path, rotated);
        }
        let level = if view.error.is_some() {
            "ERROR"
        } else if !view.missing.is_empty() {
            "WARN"
        } else {
            "INFO"
        };
        let rank = |value: &str| match value {
            "DEBUG" => 0,
            "INFO" => 1,
            "WARN" => 2,
            _ => 3,
        };
        if rank(level) < rank(&self.settings().log_level) {
            return;
        }
        let value = serde_json::json!({"time":chrono::Utc::now().to_rfc3339(),"level":level,"task_id":view.id,"attempt_id":view.attempt_id,"state":view.state,"done":view.done,"total":view.total,"bytes":view.bytes,"code":view.error.as_ref().map(|e|&e.code),"resource_id":view.error.as_ref().and_then(|e|e.resource_id.as_ref()),"diagnostic":view.error.as_ref().map(|e|&e.message)});
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{value}");
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut files = entries
                .flatten()
                .filter_map(|e| {
                    e.metadata().ok().map(|m| {
                        (
                            e.path(),
                            m.len(),
                            m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                        )
                    })
                })
                .collect::<Vec<_>>();
            files.sort_by_key(|f| f.2);
            let mut size = files.iter().map(|f| f.1).sum::<u64>();
            for (p, n, modified) in files {
                if p != path
                    && (size > 100 * 1024 * 1024
                        || modified.elapsed().is_ok_and(|d| d.as_secs() > 7 * 86400))
                {
                    if std::fs::remove_file(p).is_ok() {
                        size = size.saturating_sub(n);
                    }
                }
            }
        }
    }
    pub fn logs(&self) -> Result<String> {
        let p = self.store.root.join("logs/application.jsonl");
        if p.exists() {
            Ok(std::fs::read_to_string(p)?)
        } else {
            Ok(String::new())
        }
    }
    pub async fn diagnostics(&self) -> Diagnostics {
        let s = self.settings();
        Diagnostics {
            secure_storage: self.store.secure_available(),
            data_dir: self.store.root.to_string_lossy().into_owned(),
            cache_dir: s.cache_dir,
            capabilities: media::capabilities(&s.ffmpeg, &s.ffprobe).await.ok(),
        }
    }
    pub async fn preview(self: &Arc<Self>, input: TaskInput) -> Result<PreviewInfo> {
        let cancel = CancellationToken::new();
        let net = Network::new(
            input.network.clone(),
            &input.source,
            input.local_root.as_deref(),
            self.global.clone(),
        )?;
        let plan = plan::build(&net, &input, &cancel).await?;
        let mut keys = HashMap::new();
        for (ti, track) in plan.tracks.iter().enumerate() {
            for segment in &track.playlist.segments {
                if let Some(k) = &segment.key {
                    let context = format!("{ti}:{}", k.id);
                    if keys.contains_key(&context) {
                        continue;
                    }
                    let key = if let Some(v) = input
                        .manual_keys
                        .get(&context)
                        .or_else(|| input.manual_keys.get(&k.id.to_string()))
                    {
                        crypto::decode_key(v)?
                    } else {
                        net.get(&k.uri, None, 16, &cancel)
                            .await?
                            .bytes
                            .try_into()
                            .map_err(|_| AppError::new("KEY_INVALID", "密钥响应必须为 16 字节"))?
                    };
                    keys.insert(context, Zeroizing::new(key));
                }
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let mut resources = HashMap::new();
        let mut manifests = Vec::new();
        for (ti, track) in plan.tracks.iter().enumerate() {
            let mut text = format!(
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:{}\n",
                track.playlist.target_duration.ceil() as u64,
                track.playlist.segments[0].sequence
            );
            let mut disc = None;
            for seg in &track.playlist.segments {
                if disc.is_some_and(|d| d != seg.discontinuity) {
                    text.push_str("#EXT-X-DISCONTINUITY\n");
                }
                disc = Some(seg.discontinuity);
                let token = format!("{ti}-{}", seg.index);
                resources.insert(
                    token.clone(),
                    PreviewResource {
                        uri: seg.uri.clone(),
                        range: seg.range.clone(),
                        key: seg.key.clone(),
                        sequence: seg.sequence,
                        kind: track.kind.clone(),
                        map: seg.map.clone(),
                    },
                );
                text.push_str(&format!(
                    "#EXTINF:{},\nhttps://preview.invalid/{id}/{token}\n",
                    seg.duration
                ));
            }
            text.push_str("#EXT-X-ENDLIST\n");
            manifests.push((track.kind.clone(), text));
        }
        let mut master = "#EXTM3U\n".to_string();
        for (ti, (kind, text)) in manifests.iter().enumerate() {
            let token = format!("track-{ti}");
            resources.insert(
                token.clone(),
                PreviewResource {
                    uri: text.clone(),
                    range: None,
                    key: None,
                    sequence: 0,
                    kind: "manifest".into(),
                    map: None,
                },
            );
            if kind == "audio" {
                master.push_str(&format!("#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"Audio\",DEFAULT=YES,AUTOSELECT=YES,URI=\"https://preview.invalid/{id}/{token}\"\n"));
            }
            if kind == "subtitle" {
                master.push_str(&format!("#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"Subtitles\",DEFAULT=YES,AUTOSELECT=YES,URI=\"https://preview.invalid/{id}/{token}\"\n"));
            }
        }
        let has_audio = manifests.iter().any(|(k, _)| k == "audio");
        master.push_str(&format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={}{}{}\nhttps://preview.invalid/{id}/track-0\n",
            plan.bandwidth.max(500000),
            if has_audio { ",AUDIO=\"audio\"" } else { "" },
            if manifests.iter().any(|(k, _)| k == "subtitle") {
                ",SUBTITLES=\"subs\""
            } else {
                ""
            }
        ));
        let mut previews = self.previews.lock().unwrap();
        previews.retain(|_, p| {
            if p.created.elapsed() > Duration::from_secs(3600) {
                p.cancel.cancel();
                false
            } else {
                true
            }
        });
        if previews.len() >= 2 {
            return Err(AppError::new("LIMIT_EXCEEDED", "请先关闭已有预览"));
        }
        previews.insert(
            id.clone(),
            PreviewSession {
                net,
                cancel,
                resources,
                input,
                created: Instant::now(),
                keys: Arc::new(keys),
            },
        );
        Ok(PreviewInfo {
            id,
            manifest: master,
        })
    }
    pub async fn preview_resource(&self, id: &str, token: &str) -> Result<Vec<u8>> {
        let (net, cancel, r, input, keys) = {
            let previews = self.previews.lock().unwrap();
            let p = previews
                .get(id)
                .ok_or_else(|| AppError::new("PREVIEW_CLOSED", "预览已关闭"))?;
            let r = p
                .resources
                .get(token)
                .cloned()
                .ok_or_else(|| AppError::new("PERMISSION_DENIED", "未知预览资源"))?;
            (
                p.net.clone(),
                p.cancel.clone(),
                r,
                p.input.clone(),
                p.keys.clone(),
            )
        };
        if r.kind == "manifest" {
            return Ok(r.uri.into_bytes());
        }
        let mut bytes = net
            .get(&r.uri, r.range.as_ref(), 64 * 1024 * 1024, &cancel)
            .await?
            .bytes;
        if let Some(k) = r.key {
            let ti = token.split('-').next().unwrap_or("0");
            let context = format!("{ti}:{}", k.id);
            let key = keys
                .get(&context)
                .ok_or_else(|| AppError::new("KEY_INVALID", "密钥上下文不存在"))?;
            let iv = input
                .manual_ivs
                .get(&context)
                .or_else(|| input.manual_ivs.get(&k.id.to_string()))
                .map(String::as_str)
                .or(k.iv.as_deref());
            bytes = crypto::decrypt(&bytes, key, &crypto::iv(iv, r.sequence)?)?;
        } else if let Some(key) = forced_key(
            &input,
            token
                .split('-')
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            r.sequence,
        ) {
            bytes = crypto::decrypt(
                &bytes,
                &crypto::decode_key(key)?,
                &crypto::iv(
                    input.manual_ivs.get("manual").map(String::as_str),
                    r.sequence,
                )?,
            )?;
        }
        if r.kind == "subtitle" {
            if let Some(map) = r.map {
                let mut header = net.get(&map, None, 1024 * 1024, &cancel).await?.bytes;
                header.extend_from_slice(b"\n\n");
                header.extend(bytes);
                bytes = header;
            }
        }
        crypto::validate_media(&bytes, r.kind == "subtitle")?;
        Ok(bytes)
    }
    pub fn close_preview(&self, id: &str) {
        if let Some(p) = self.previews.lock().unwrap().remove(id) {
            p.cancel.cancel();
        }
        if let Some((cancel, _)) = self.local_previews.lock().unwrap().remove(id) {
            cancel.cancel();
        }
    }
    pub async fn local_preview(&self, id: &str, path: &Path) -> Result<String> {
        uuid::Uuid::parse_str(id).map_err(|_| AppError::new("INPUT_INVALID", "预览 ID 无效"))?;
        let path = std::fs::canonicalize(path)?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !["ts", "mkv", "mp4"].contains(&ext.as_str()) {
            return Err(AppError::new("INPUT_INVALID", "请选择 TS、MP4 或 MKV 媒体"));
        }
        if ext == "mp4" {
            return Ok(path.to_string_lossy().into_owned());
        }
        let dir = tempfile::Builder::new()
            .prefix("local-preview-")
            .tempdir_in(&self.store.root)?;
        let output = dir.path().join("preview.mp4");
        let cancel = CancellationToken::new();
        {
            let mut sessions = self.local_previews.lock().unwrap();
            if sessions.len() >= 2 || sessions.contains_key(id) {
                return Err(AppError::new("LIMIT_EXCEEDED", "请先关闭已有本地预览"));
            }
            sessions.insert(id.into(), (cancel.clone(), dir));
        }
        let settings = self.settings();
        let result =
            media::prepare_local(&settings.ffmpeg, &settings.ffprobe, &path, &output, &cancel)
                .await;
        if result.is_err() {
            self.close_preview(id);
        }
        result?;
        Ok(output.to_string_lossy().into_owned())
    }
}
fn needs_credentials(input: &TaskInput) -> bool {
    !input.remember
        && (!input.network.headers.is_empty()
            || !input.network.origin_headers.is_empty()
            || !input.manual_keys.is_empty()
            || !input.manual_ivs.is_empty()
            || input.force_key.is_some()
            || url::Url::parse(&input.network.proxy).is_ok_and(|u| u.password().is_some()))
}
fn plan_identity(plan: &Plan) -> Result<String> {
    Ok(hash(&serde_json::to_vec(
        &plan
            .tracks
            .iter()
            .map(|t| (&t.kind, &t.playlist.segments))
            .collect::<Vec<_>>(),
    )?))
}
fn segment_identity(ti: usize, segment: &Segment, input: &TaskInput) -> Result<String> {
    let (key, iv) = if let Some(k) = &segment.key {
        let context = format!("{ti}:{}", k.id);
        (
            input
                .manual_keys
                .get(&context)
                .or_else(|| input.manual_keys.get(&k.id.to_string())),
            input
                .manual_ivs
                .get(&context)
                .or_else(|| input.manual_ivs.get(&k.id.to_string())),
        )
    } else {
        (
            forced_key(input, ti, segment.sequence),
            if forced_key(input, ti, segment.sequence).is_some() {
                input.manual_ivs.get("manual")
            } else {
                None
            },
        )
    };
    Ok(hash(&serde_json::to_vec(&(segment, key, iv))?))
}
fn forced_key(input: &TaskInput, ti: usize, sequence: u64) -> Option<&String> {
    if ti == input.force_track
        && input
            .force_range
            .is_some_and(|r| sequence >= r[0] && sequence <= r[1])
    {
        input.force_key.as_ref()
    } else {
        None
    }
}
pub fn safe_remove_tree(root: &Path, path: &Path) -> Result<()> {
    let root = std::fs::canonicalize(root)?;
    let canonical = std::fs::canonicalize(path)?;
    if canonical == root
        || !canonical.starts_with(&root)
        || std::fs::symlink_metadata(path)?.file_type().is_symlink()
    {
        return Err(AppError::new(
            "PERMISSION_DENIED",
            "拒绝清理缓存根目录外的数据",
        ));
    }
    crate::filesystem::check_tree(&root, path)?;
    std::fs::remove_dir_all(path)?;
    Ok(())
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;

    #[test]
    fn settings_keep_total_budget_while_preview_holds_permits() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        let held = engine.global.clone().try_acquire_many_owned(8).unwrap();
        let old = engine.settings();
        let mut new = old.clone();
        new.theme = "dark".into();
        engine.save_settings(new.clone()).unwrap();
        assert_eq!(engine.global.available_permits(), old.max_requests - 8);
        new.max_requests = 4;
        assert_eq!(
            engine.save_settings(new.clone()).unwrap_err().code,
            "TASK_BUSY"
        );
        drop(held);
        assert_eq!(engine.global.available_permits(), old.max_requests);
        engine.save_settings(new).unwrap();
        assert_eq!(engine.global.available_permits(), 4);
    }
}
