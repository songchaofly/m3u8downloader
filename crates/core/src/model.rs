use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ByteRange {
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyContext {
    pub id: usize,
    pub uri: String,
    pub iv: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub index: usize,
    pub sequence: u64,
    pub uri: String,
    pub duration: f64,
    pub range: Option<ByteRange>,
    pub discontinuity: u64,
    pub key: Option<KeyContext>,
    pub map: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variant {
    pub uri: String,
    pub bandwidth: u64,
    pub resolution: Option<String>,
    pub codecs: Option<String>,
    pub audio: Option<String>,
    pub subtitles: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rendition {
    pub kind: String,
    pub group: String,
    pub name: String,
    pub language: Option<String>,
    pub uri: Option<String>,
    pub default: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub source: String,
    pub master: bool,
    pub ended: bool,
    pub target_duration: f64,
    pub segments: Vec<Segment>,
    pub variants: Vec<Variant>,
    pub renditions: Vec<Rendition>,
    pub warnings: Vec<String>,
    pub raw: String,
}
impl Playlist {
    pub fn duration(&self) -> f64 {
        self.segments.iter().map(|s| s.duration).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    pub user_agent: String,
    pub headers: BTreeMap<String, String>,
    pub origin_headers: BTreeMap<String, BTreeMap<String, String>>,
    pub proxy: String,
    pub connect_timeout: u64,
    pub read_timeout: u64,
    pub request_timeout: u64,
    pub retries: u32,
    pub concurrency: usize,
    pub limit_kib: u64,
    pub max_redirects: usize,
}
impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            user_agent: "M3U8Studio/0.1".into(),
            headers: BTreeMap::new(),
            origin_headers: BTreeMap::new(),
            proxy: "direct".into(),
            connect_timeout: 10,
            read_timeout: 30,
            request_timeout: 300,
            retries: 3,
            concurrency: 8,
            limit_kib: 0,
            max_redirects: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskInput {
    pub source: String,
    pub title: String,
    pub output_dir: String,
    pub container: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub variant: Option<usize>,
    pub audio: Option<usize>,
    pub subtitle: Option<usize>,
    pub video_only: bool,
    pub subtitle_mode: String,
    pub filename_template: String,
    pub network: NetworkConfig,
    pub manual_keys: BTreeMap<String, String>,
    pub manual_ivs: BTreeMap<String, String>,
    pub force_key: Option<String>,
    pub force_track: usize,
    pub force_range: Option<[u64; 2]>,
    pub allow_gaps: bool,
    pub confirm_gaps: bool,
    pub keep_cache: bool,
    pub remember: bool,
    pub local_root: Option<String>,
    pub base_url: Option<String>,
    pub ffmpeg: String,
    pub ffprobe: String,
    pub fast_ts: bool,
    pub overwrite: bool,
}
impl Default for TaskInput {
    fn default() -> Self {
        Self {
            source: String::new(),
            title: String::new(),
            output_dir: String::new(),
            container: "mp4".into(),
            video_codec: "copy".into(),
            audio_codec: "copy".into(),
            variant: None,
            audio: None,
            subtitle: None,
            video_only: false,
            subtitle_mode: "sidecar".into(),
            filename_template: "{title}_{resolution}_{task_id}".into(),
            network: NetworkConfig::default(),
            manual_keys: BTreeMap::new(),
            manual_ivs: BTreeMap::new(),
            force_key: None,
            force_track: 0,
            force_range: None,
            allow_gaps: false,
            confirm_gaps: false,
            keep_cache: false,
            remember: false,
            local_root: None,
            base_url: None,
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
            fast_ts: false,
            overwrite: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub kind: String,
    pub language: Option<String>,
    pub playlist: Playlist,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub tracks: Vec<Track>,
    pub resolution: String,
    pub bandwidth: u64,
    pub duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub source_display: String,
    pub state: String,
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub speed: u64,
    pub progress: f64,
    pub resolution: String,
    pub output: Option<String>,
    pub error: Option<crate::AppError>,
    pub created_at: String,
    pub attempt_id: String,
    pub event_seq: u64,
    pub missing: Vec<String>,
    pub needs_credentials: bool,
    #[serde(default)]
    pub processing_ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub output_dir: String,
    pub cache_dir: String,
    pub theme: String,
    pub language: String,
    pub network: NetworkConfig,
    pub max_tasks: usize,
    pub max_requests: usize,
    pub max_processing: usize,
    pub low_disk_mib: u64,
    pub keep_cache: bool,
    pub notify: bool,
    pub history_days: u64,
    pub log_level: String,
    pub schema_version: u32,
    pub ffmpeg: String,
    pub ffprobe: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: String::new(),
            cache_dir: String::new(),
            theme: "system".into(),
            language: "zh-CN".into(),
            network: NetworkConfig::default(),
            max_tasks: 2,
            max_requests: 16,
            max_processing: 1,
            low_disk_mib: 1024,
            keep_cache: false,
            notify: true,
            history_days: 0,
            log_level: "INFO".into(),
            schema_version: 1,
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        }
    }
}
