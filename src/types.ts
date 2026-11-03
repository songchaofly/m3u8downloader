export interface NetworkConfig {
  user_agent: string;
  headers: Record<string, string>;
  origin_headers: Record<string, Record<string, string>>;
  proxy: string;
  connect_timeout: number;
  read_timeout: number;
  request_timeout: number;
  retries: number;
  concurrency: number;
  limit_kib: number;
  max_redirects: number;
}
export interface TaskInput {
  source: string;
  title: string;
  output_dir: string;
  container: string;
  video_codec: string;
  audio_codec: string;
  variant: number | null;
  audio: number | null;
  subtitle: number | null;
  video_only: boolean;
  subtitle_mode: string;
  filename_template: string;
  network: NetworkConfig;
  manual_keys: Record<string, string>;
  manual_ivs: Record<string, string>;
  force_key: string | null;
  force_track: number;
  force_range: [number, number] | null;
  confirm_gaps: boolean;
  allow_gaps: boolean;
  keep_cache: boolean;
  remember: boolean;
  local_root: string | null;
  base_url: string | null;
  ffmpeg: string;
  ffprobe: string;
  fast_ts: boolean;
  overwrite: boolean;
}
export interface Task {
  id: string;
  title: string;
  source_display: string;
  state: string;
  stage: string;
  done: number;
  total: number;
  bytes: number;
  speed: number;
  progress: number;
  resolution: string;
  output: string | null;
  error: { code: string; message: string } | null;
  created_at: string;
  attempt_id: string;
  event_seq: number;
  missing: string[];
  needs_credentials: boolean;
}
export interface Settings {
  output_dir: string;
  cache_dir: string;
  theme: string;
  language: string;
  network: NetworkConfig;
  max_tasks: number;
  max_requests: number;
  max_processing: number;
  low_disk_mib: number;
  keep_cache: boolean;
  notify: boolean;
  history_days: number;
  log_level: string;
  schema_version: number;
  ffmpeg: string;
  ffprobe: string;
}
export interface Playlist {
  source: string;
  master: boolean;
  ended: boolean;
  target_duration: number;
  segments: {
    index: number;
    sequence: number;
    uri: string;
    duration: number;
    key: { id: number; uri: string; iv: string | null } | null;
    range: { offset: number; length: number } | null;
  }[];
  variants: {
    uri: string;
    bandwidth: number;
    resolution: string | null;
    codecs: string | null;
    audio: string | null;
    subtitles: string | null;
  }[];
  renditions: {
    kind: string;
    group: string;
    name: string;
    language: string | null;
    uri: string | null;
    default: boolean;
  }[];
  warnings: string[];
  raw: string;
}
export interface Diagnostics {
  secure_storage: boolean;
  data_dir: string;
  cache_dir: string;
  capabilities: {
    ffmpeg: string;
    ffprobe: string;
    h264: boolean;
    h265: boolean;
    aac: boolean;
  } | null;
}
export const networkDefaults = (): NetworkConfig => ({
  user_agent: "M3U8Studio/0.1",
  headers: {},
  origin_headers: {},
  proxy: "direct",
  connect_timeout: 10,
  read_timeout: 30,
  request_timeout: 300,
  retries: 3,
  concurrency: 8,
  limit_kib: 0,
  max_redirects: 5,
});
export const inputDefaults = (): TaskInput => ({
  source: "",
  title: "",
  output_dir: "",
  container: "mp4",
  video_codec: "copy",
  audio_codec: "copy",
  variant: null,
  audio: null,
  subtitle: null,
  video_only: false,
  subtitle_mode: "sidecar",
  filename_template: "{title}_{resolution}_{task_id}",
  network: networkDefaults(),
  manual_keys: {},
  manual_ivs: {},
  force_key: null,
  force_track: 0,
  force_range: null,
  confirm_gaps: false,
  allow_gaps: false,
  keep_cache: false,
  remember: false,
  local_root: null,
  base_url: null,
  ffmpeg: "ffmpeg",
  ffprobe: "ffprobe",
  fast_ts: false,
  overwrite: false,
});

export interface TaskDetails {
  total: number;
  segments: {
    resource: string;
    track: string;
    sequence: number;
    duration: number;
    state: string;
    bytes: number;
    source: string;
    context: string | null;
    iv: string | null;
    discontinuity: number;
    range: { offset: number; length: number } | null;
  }[];
  attempts: Task[];
}
