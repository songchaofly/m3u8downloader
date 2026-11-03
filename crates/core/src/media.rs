use crate::{AppError, Plan, Result, TaskInput};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub ffmpeg: String,
    pub ffprobe: String,
    pub h264: bool,
    pub h265: bool,
    pub aac: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputResult {
    pub path: String,
    pub bytes: u64,
    pub duration: f64,
    pub streams: Vec<String>,
    pub subtitle: Option<String>,
}

struct Reservation(PathBuf);
impl Drop for Reservation {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub struct PreparedOutput {
    _staging: tempfile::TempDir,
    _reservation: Reservation,
    files: Vec<(PathBuf, PathBuf)>,
    result: OutputResult,
    overwrite: bool,
    cache_root: PathBuf,
    local_inputs: Vec<PathBuf>,
}
impl PreparedOutput {
    /// Called under the task's state lock: cancellation and publication have one order.
    pub fn publish(self, cancel: &CancellationToken) -> Result<OutputResult> {
        if cancel.is_cancelled() {
            return Err(AppError::cancelled());
        }
        for (_, target) in &self.files {
            protect_output(target, &self.cache_root, &self.local_inputs)?;
        }
        let mut committed: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        for (source, target) in &self.files {
            let attempt = (|| -> Result<Option<PathBuf>> {
                let backup = if self.overwrite && target.exists() {
                    let parent = std::fs::canonicalize(target.parent().unwrap())?;
                    crate::filesystem::check_tree(&parent, target)?;
                    if !target.is_file() {
                        return Err(AppError::new("OUTPUT_CONFLICT", "覆盖目标不是普通文件"));
                    }
                    let backup = self
                        ._staging
                        .path()
                        .join(format!("backup-{}", uuid::Uuid::new_v4()));
                    if std::fs::hard_link(target, &backup).is_err() {
                        std::fs::copy(target, &backup)?;
                    }
                    Some(backup)
                } else {
                    None
                };
                let temp = tempfile::TempPath::try_from_path(source)?;
                let result = if self.overwrite {
                    temp.persist(target)
                } else {
                    temp.persist_noclobber(target)
                };
                result.map_err(|e| {
                    AppError::new(
                        "OUTPUT_CONFLICT",
                        format!("输出提交失败：{}", e.error.kind()),
                    )
                })?;
                Ok(backup)
            })();
            match attempt {
                Ok(backup) => committed.push((target.clone(), backup)),
                Err(error) => {
                    for (path, backup) in committed.into_iter().rev() {
                        if let Some(backup) = backup {
                            if let Ok(temp) = tempfile::TempPath::try_from_path(backup) {
                                let _ = temp.persist(&path);
                            }
                        } else {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                    return Err(error);
                }
            }
        }
        Ok(self.result)
    }
}

fn protect_output(target: &Path, cache_root: &Path, local_inputs: &[PathBuf]) -> Result<()> {
    let parent = std::fs::canonicalize(target.parent().unwrap())?;
    let resolved = parent.join(target.file_name().unwrap());
    if resolved.starts_with(cache_root) || local_inputs.iter().any(|input| input == &resolved) {
        return Err(AppError::new(
            "OUTPUT_CONFLICT",
            "输出不能覆盖输入资源或写入任务缓存目录",
        ));
    }
    if std::fs::symlink_metadata(target).is_ok() {
        crate::filesystem::check_tree(&parent, target)?;
        if local_inputs
            .iter()
            .any(|input| std::fs::canonicalize(target).is_ok_and(|p| &p == input))
        {
            return Err(AppError::new("OUTPUT_CONFLICT", "输出不能覆盖输入资源"));
        }
    }
    Ok(())
}

fn command(exe: &str) -> Command {
    let mut c = Command::new(exe);
    c.kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(0x08000000);
    c
}
fn ffpath(path: &Path) -> String {
    let path = path.to_string_lossy();
    // FFmpeg resolves HLS relative URIs incorrectly for Rust's Windows verbatim prefix.
    #[cfg(windows)]
    {
        path.strip_prefix(r"\\?\")
            .unwrap_or(&path)
            .replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        path.into_owned()
    }
}
pub async fn capabilities(ffmpeg: &str, ffprobe: &str) -> Result<Capabilities> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        command(ffmpeg).args(["-hide_banner", "-encoders"]).output(),
    )
    .await
    .map_err(|_| AppError::new("FFMPEG_UNAVAILABLE", "FFmpeg 检查超时"))??;
    if !out.status.success() {
        return Err(AppError::new("FFMPEG_UNAVAILABLE", "无法运行 FFmpeg"));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let probe = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        command(ffprobe).arg("-version").output(),
    )
    .await
    .map_err(|_| AppError::new("FFMPEG_UNAVAILABLE", "ffprobe 检查超时"))??;
    if !probe.status.success() {
        return Err(AppError::new("FFMPEG_UNAVAILABLE", "无法运行 ffprobe"));
    }
    Ok(Capabilities {
        ffmpeg: ffmpeg.into(),
        ffprobe: ffprobe.into(),
        h264: text.contains("libx264"),
        h265: text.contains("libx265"),
        aac: text
            .lines()
            .any(|l| l.split_whitespace().nth(1) == Some("aac")),
    })
}
pub fn safe_name(value: &str) -> String {
    let cleaned = value
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();
    let cleaned = cleaned
        .trim_matches([' ', '.'])
        .chars()
        .take(100)
        .collect::<String>();
    let upper = cleaned.split('.').next().unwrap_or("").to_uppercase();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ]
    .contains(&upper.as_str());
    if cleaned.is_empty() {
        "media".into()
    } else if reserved {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}
pub fn validate_input(input: &TaskInput) -> Result<()> {
    if !["mp4", "mkv", "ts"].contains(&input.container.as_str())
        || !["copy", "h264", "h265"].contains(&input.video_codec.as_str())
        || !["copy", "aac"].contains(&input.audio_codec.as_str())
        || !["sidecar", "embed"].contains(&input.subtitle_mode.as_str())
    {
        return Err(AppError::new("INPUT_INVALID", "输出容器或编码配置无效"));
    }
    if input.filename_template.contains(['/', '\\'])
        || input.filename_template.contains("..")
        || input.filename_template.len() > 200
    {
        return Err(AppError::new("INPUT_INVALID", "命名模板不可包含路径或过长"));
    }
    if input.output_dir.trim().is_empty() {
        return Err(AppError::new("INPUT_INVALID", "请先选择输出目录"));
    }
    for key in input.manual_keys.values() {
        crate::crypto::decode_key(key)?;
    }
    if let Some(key) = &input.force_key {
        crate::crypto::decode_key(key)?;
        if input.force_range.is_none_or(|r| r[0] > r[1]) {
            return Err(AppError::new(
                "INPUT_INVALID",
                "手动 AES-128 必须指定有效的媒体序号区间",
            ));
        }
    }
    for iv in input.manual_ivs.values() {
        crate::crypto::iv(Some(iv), 0)?;
    }
    Ok(())
}

fn stamp(v: f64) -> String {
    let ms = (v.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3600000,
        ms / 60000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}
fn seconds(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let p = s.split(':').collect::<Vec<_>>();
    match p.as_slice() {
        [h, m, s] => Some(
            h.parse::<f64>().ok()? * 3600.0
                + m.parse::<f64>().ok()? * 60.0
                + s.parse::<f64>().ok()?,
        ),
        [m, s] => Some(m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?),
        _ => None,
    }
}
pub fn merge_vtt(parts: &[(String, f64)]) -> Result<String> {
    let mut cues = Vec::<(f64, f64, String)>::new();
    let mut first_mpeg: Option<f64> = None;
    for (text, elapsed) in parts {
        let normalized = text.replace("\r\n", "\n");
        let mut offset = 0.0;
        if let Some(map) = normalized
            .lines()
            .find(|l| l.starts_with("X-TIMESTAMP-MAP="))
        {
            let fields = map
                .trim_start_matches("X-TIMESTAMP-MAP=")
                .split(',')
                .collect::<Vec<_>>();
            let local = fields
                .iter()
                .find_map(|f| f.strip_prefix("LOCAL:").and_then(seconds))
                .ok_or_else(|| AppError::new("SEGMENT_INVALID", "字幕时间映射缺少 LOCAL"))?;
            let mpeg = fields
                .iter()
                .find_map(|f| {
                    f.strip_prefix("MPEGTS:")
                        .and_then(|v| v.parse::<f64>().ok())
                })
                .ok_or_else(|| AppError::new("SEGMENT_INVALID", "字幕时间映射缺少 MPEGTS"))?
                / 90000.0;
            let origin = *first_mpeg.get_or_insert(mpeg - local - *elapsed);
            offset = mpeg - local - origin;
        }
        for block in normalized.split("\n\n") {
            let lines = block.lines().collect::<Vec<_>>();
            if let Some(i) = lines.iter().position(|l| l.contains("-->")) {
                let (a, b) = lines[i].split_once("-->").unwrap();
                let a = seconds(a)
                    .ok_or_else(|| AppError::new("SEGMENT_INVALID", "字幕开始时间无效"))?;
                let b = seconds(b.split_whitespace().next().unwrap_or(""))
                    .ok_or_else(|| AppError::new("SEGMENT_INVALID", "字幕结束时间无效"))?;
                if b < a {
                    return Err(AppError::new("SEGMENT_INVALID", "字幕结束时间早于开始时间"));
                }
                let payload = lines[i + 1..].join("\n");
                let cue = (a + offset, b + offset, payload);
                if !cues.iter().any(|c| {
                    (c.0 - cue.0).abs() < 0.001 && (c.1 - cue.1).abs() < 0.001 && c.2 == cue.2
                }) {
                    cues.push(cue);
                }
            }
        }
    }
    cues.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = "WEBVTT\n\n".to_string();
    for (a, b, payload) in cues {
        out.push_str(&format!("{} --> {}\n{}\n\n", stamp(a), stamp(b), payload));
    }
    Ok(out)
}

pub async fn process(
    input: &TaskInput,
    plan: &Plan,
    cache: &Path,
    id: &str,
    created: &str,
    missing: &[String],
    cancel: &CancellationToken,
    progress: impl Fn(f64) + Send + Sync,
) -> Result<PreparedOutput> {
    validate_input(input)?;
    let output_dir = PathBuf::from(&input.output_dir);
    tokio::fs::create_dir_all(&output_dir).await?;
    let cache_root = std::fs::canonicalize(
        cache
            .parent()
            .ok_or_else(|| AppError::new("OUTPUT_CONFLICT", "缓存根目录无效"))?,
    )?;
    let mut local_inputs = Vec::new();
    let mut protect = |uri: &str| {
        if let Ok(url) = crate::playlist::source_url(uri) {
            if let Ok(path) = url.to_file_path() {
                if let Ok(path) = std::fs::canonicalize(path) {
                    local_inputs.push(path);
                }
            }
        }
    };
    protect(&input.source);
    for track in &plan.tracks {
        protect(&track.playlist.source);
        for segment in &track.playlist.segments {
            protect(&segment.uri);
            if let Some(key) = &segment.key {
                protect(&key.uri);
            }
            if let Some(map) = &segment.map {
                protect(map);
            }
        }
    }
    let title = if input.title.trim().is_empty() {
        "video"
    } else {
        input.title.trim()
    };
    let date = chrono::DateTime::parse_from_rfc3339(created)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y%m%d-%H%M%S")
                .to_string()
        })
        .unwrap_or_else(|_| "unknown".into());
    let name = input
        .filename_template
        .replace("{title}", &safe_name(title))
        .replace("{resolution}", &plan.resolution)
        .replace("{bitrate}", &plan.bandwidth.to_string())
        .replace("{datetime}", &date)
        .replace("{task_id}", &id[..8]);
    if name.contains(['{', '}']) {
        return Err(AppError::new("INPUT_INVALID", "文件名模板包含未知变量"));
    }
    let name = format!(
        "{}{}",
        safe_name(&name),
        if missing.is_empty() {
            ""
        } else {
            ".incomplete"
        }
    );
    let mut n = 0;
    let (output, reservation) = loop {
        let name = if n == 0 {
            name.clone()
        } else {
            format!("{name} ({n})")
        };
        let output = output_dir.join(format!("{name}.{}", input.container));
        protect_output(&output, &cache_root, &local_inputs)?;
        let lock = output_dir.join(format!("{name}.reserve"));
        let conflict = ["mp4", "mkv", "ts", "vtt", "missing.json"]
            .iter()
            .any(|ext| std::fs::symlink_metadata(output.with_extension(ext)).is_ok());
        if !conflict || input.overwrite {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock)
            {
                Ok(_) => break (output, lock),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        n += 1;
        if n > 10000 {
            return Err(AppError::new("OUTPUT_CONFLICT", "无法找到可用输出名称"));
        }
    };
    let reservation = Reservation(reservation);
    let staging = tempfile::Builder::new()
        .prefix(".m3u8-stage-")
        .tempdir_in(&output_dir)?;
    let temporary = staging.path().join(format!("media.{}", input.container));
    let mut inputs = Vec::new();
    let mut subtitle = None;
    for (ti, track) in plan.tracks.iter().enumerate() {
        if track.kind == "subtitle" {
            let mut parts = Vec::new();
            let mut elapsed = 0.0;
            for segment in &track.playlist.segments {
                let file = cache.join(format!("{ti}-{}.vtt", segment.index));
                parts.push((tokio::fs::read_to_string(file).await?, elapsed));
                elapsed += segment.duration;
            }
            let text = merge_vtt(&parts)?;
            let path = staging.path().join("subtitles.vtt");
            tokio::fs::write(&path, text).await?;
            subtitle = Some(path);
            continue;
        }
        let path = cache.join(format!("track-{ti}.m3u8"));
        let mut text = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-PLAYLIST-TYPE:VOD\n",
            track.playlist.target_duration.ceil() as u64
        );
        let mut last_disc = None;
        for segment in &track.playlist.segments {
            if missing.contains(&format!("{ti}:{}", segment.index)) {
                continue;
            }
            if last_disc.is_some_and(|d| d != segment.discontinuity) {
                text.push_str("#EXT-X-DISCONTINUITY\n");
            }
            last_disc = Some(segment.discontinuity);
            text.push_str(&format!(
                "#EXTINF:{},\n{ti}-{}.ts\n",
                segment.duration, segment.index
            ));
        }
        text.push_str("#EXT-X-ENDLIST\n");
        crate::filesystem::atomic_write(&path, text.as_bytes())?;
        inputs.push((track.kind.clone(), path));
    }
    if input.fast_ts {
        if input.container != "ts"
            || input.video_only
            || inputs.len() != 1
            || input.video_codec != "copy"
            || input.audio_codec != "copy"
            || plan.tracks[0]
                .playlist
                .segments
                .windows(2)
                .any(|s| s[0].discontinuity != s[1].discontinuity)
        {
            return Err(AppError::new(
                "OUTPUT_INCOMPATIBLE",
                "快速 TS 拼接要求单一轨道组、无断点、不重新编码且不移除音轨",
            ));
        }
        let mut out = tokio::fs::File::create(&temporary).await?;
        let mut structure = None;
        for segment in &plan.tracks[0].playlist.segments {
            if cancel.is_cancelled() {
                return Err(AppError::cancelled());
            }
            if missing.contains(&format!("0:{}", segment.index)) {
                continue;
            }
            let path = cache.join(format!("0-{}.ts", segment.index));
            let current = stream_structure(&input.ffprobe, &path, cancel).await?;
            if structure.as_ref().is_some_and(|old| old != &current) {
                return Err(AppError::new(
                    "OUTPUT_INCOMPATIBLE",
                    "快速 TS 模式检测到编码或流结构变化，请使用常规封装或转码",
                ));
            }
            structure = Some(current);
            let mut f = tokio::fs::File::open(path).await?;
            tokio::io::copy(&mut f, &mut out).await?;
        }
        out.flush().await?;
        out.sync_all().await?;
    } else {
        let mut args = vec![
            "-hide_banner".into(),
            "-nostdin".into(),
            "-y".into(),
            "-loglevel".into(),
            "error".into(),
        ];
        for (_, p) in &inputs {
            args.extend([
                "-protocol_whitelist".into(),
                "file".into(),
                "-i".into(),
                ffpath(p),
            ]);
        }
        let embed = subtitle.is_some() && input.subtitle_mode == "embed" && input.container != "ts";
        if embed {
            args.extend([
                "-protocol_whitelist".into(),
                "file".into(),
                "-i".into(),
                ffpath(subtitle.as_ref().unwrap()),
            ]);
        }
        args.extend(["-map".into(), "0:v:0?".into()]);
        if !input.video_only {
            let index = inputs.iter().position(|(k, _)| k == "audio").unwrap_or(0);
            args.extend(["-map".into(), format!("{index}:a:0?")]);
        }
        args.extend([
            "-c:v".into(),
            match input.video_codec.as_str() {
                "h264" => "libx264",
                "h265" => "libx265",
                _ => "copy",
            }
            .into(),
            "-c:a".into(),
            input.audio_codec.clone(),
        ]);
        if input.video_codec != "copy" {
            args.extend([
                "-preset".into(),
                "medium".into(),
                "-crf".into(),
                "23".into(),
            ]);
        }
        if embed {
            args.extend([
                "-map".into(),
                format!("{}:s:0", inputs.len()),
                "-c:s".into(),
                if input.container == "mp4" {
                    "mov_text"
                } else {
                    "webvtt"
                }
                .into(),
            ]);
        }
        if input.container == "mp4" {
            args.extend(["-movflags".into(), "+faststart".into()]);
        }
        args.extend([
            "-progress".into(),
            "pipe:1".into(),
            "-nostats".into(),
            ffpath(&temporary),
        ]);
        run_ffmpeg(&input.ffmpeg, &args, cancel, plan.duration, &progress).await?;
    }
    let mut result = probe(&input.ffprobe, &temporary, cancel).await?;
    if result.streams.is_empty() || result.duration <= 0.0 || result.bytes == 0 {
        return Err(AppError::new("OUTPUT_INVALID", "输出缺少有效媒体内容"));
    }
    if plan.tracks.iter().any(|t| t.kind == "audio") && !result.streams.iter().any(|s| s == "audio")
    {
        return Err(AppError::new("OUTPUT_INVALID", "选定音轨未写入输出"));
    }
    if subtitle.is_some()
        && input.subtitle_mode == "embed"
        && input.container != "ts"
        && !result.streams.iter().any(|s| s == "subtitle")
    {
        return Err(AppError::new("OUTPUT_INVALID", "选定字幕未写入输出"));
    }
    if missing.is_empty()
        && (result.duration - plan.duration).abs() > 1.0f64.max(plan.duration * 0.005)
    {
        return Err(AppError::new(
            "OUTPUT_INVALID",
            format!(
                "输出时长 {:.2}s 与预期 {:.2}s 不一致",
                result.duration, plan.duration
            ),
        ));
    }
    // Decode a bounded initial sample; the acceptance suite performs complete decoding.
    let check = vec![
        "-v".into(),
        "error".into(),
        "-xerror".into(),
        "-nostdin".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-i".into(),
        ffpath(&temporary),
        "-t".into(),
        "3".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ];
    run_ffmpeg(&input.ffmpeg, &check, cancel, 3.0, &|_| {}).await?;
    if cancel.is_cancelled() {
        return Err(AppError::cancelled());
    }
    let mut files = Vec::new();
    if let Some(sub) = subtitle {
        if input.subtitle_mode == "sidecar" || input.container == "ts" {
            let dest = output.with_extension("vtt");
            result.subtitle = Some(dest.to_string_lossy().into_owned());
            files.push((sub, dest));
        }
    }
    if !missing.is_empty() {
        let report = staging.path().join("missing.json");
        tokio::fs::write(
            &report,
            serde_json::to_vec_pretty(&serde_json::json!({"task_id":id,"missing":missing}))?,
        )
        .await?;
        files.push((report, output.with_extension("missing.json")));
    }
    files.push((temporary, output.clone()));
    result.path = output.to_string_lossy().into_owned();
    progress(1.0);
    Ok(PreparedOutput {
        _staging: staging,
        _reservation: reservation,
        files,
        result,
        overwrite: input.overwrite,
        cache_root,
        local_inputs,
    })
}

async fn stream_structure(
    ffprobe: &str,
    path: &Path,
    cancel: &CancellationToken,
) -> Result<serde_json::Value> {
    let output = tokio::select! {
        _ = cancel.cancelled() => return Err(AppError::cancelled()),
        output = command(ffprobe).args(["-v", "error", "-protocol_whitelist", "file", "-show_entries", "stream=codec_name,codec_type,profile,width,height,pix_fmt,sample_rate,channels,channel_layout", "-of", "json"]).arg(path).output() => output?
    };
    if !output.status.success() {
        return Err(AppError::new("OUTPUT_INVALID", "无法检查分片编码和流结构"));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(value["streams"].clone())
}

pub async fn probe(ffprobe: &str, path: &Path, cancel: &CancellationToken) -> Result<OutputResult> {
    let output = tokio::select! {_=cancel.cancelled()=>return Err(AppError::cancelled()),v=command(ffprobe).args(["-v","error","-protocol_whitelist","file","-show_streams","-show_format","-of","json"]).arg(path).output()=>v?};
    if !output.status.success() {
        return Err(AppError::new("OUTPUT_INVALID", "ffprobe 无法读取媒体"));
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let streams = json["streams"]
        .as_array()
        .map(|s| {
            s.iter()
                .filter_map(|v| v["codec_type"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let duration = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    Ok(OutputResult {
        path: path.to_string_lossy().into_owned(),
        bytes: tokio::fs::metadata(path).await?.len(),
        duration,
        streams,
        subtitle: None,
    })
}
pub async fn prepare_local(
    ffmpeg: &str,
    ffprobe: &str,
    source: &Path,
    output: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    let info = probe(ffprobe, source, cancel).await?;
    let args = [
        "-hide_banner",
        "-nostdin",
        "-y",
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .map(str::to_string)
    .into_iter()
    .chain([
        ffpath(source),
        "-map".into(),
        "0:v:0?".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-c".into(),
        "copy".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-progress".into(),
        "pipe:1".into(),
        ffpath(output),
    ])
    .collect::<Vec<_>>();
    run_ffmpeg(ffmpeg, &args, cancel, info.duration, &|_| {}).await?;
    probe(ffprobe, output, cancel).await?;
    Ok(())
}

async fn run_ffmpeg(
    exe: &str,
    args: &[String],
    cancel: &CancellationToken,
    duration: f64,
    progress: &(impl Fn(f64) + Send + Sync),
) -> Result<()> {
    let mut child = command(exe)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| AppError::new("FFMPEG_UNAVAILABLE", "无法启动 FFmpeg，请检查路径"))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let errors = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut input = stderr;
        let mut buffer = [0; 4096];
        loop {
            match input.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if bytes.len() < 32768 {
                        bytes.extend_from_slice(&buffer[..n.min(32768 - bytes.len())]);
                    }
                }
            }
        }
        bytes
    });
    let mut lines = BufReader::new(stdout).lines();
    loop {
        tokio::select! {_=cancel.cancelled()=>{let _=child.kill().await;let _=child.wait().await;let _=errors.await;return Err(AppError::cancelled());},line=lines.next_line()=>{match line?{None=>break,Some(line)=>if let Some(v)=line.strip_prefix("out_time_us="){if let Ok(micros)=v.parse::<f64>(){progress((micros/1e6/duration.max(0.001)).clamp(0.0,0.99));}}}}}
    }
    let status = tokio::select! {_=cancel.cancelled()=>{let _=child.kill().await;child.wait().await?},s=child.wait()=>s?};
    let diagnostics = errors.await.unwrap_or_default();
    if cancel.is_cancelled() {
        return Err(AppError::cancelled());
    }
    if !status.success() {
        return Err(AppError::new(
            "OUTPUT_INCOMPATIBLE",
            format!(
                "FFmpeg 处理失败；缓存已保留，可修改输出配置后重试。{}",
                String::from_utf8_lossy(&diagnostics)
                    .lines()
                    .take(6)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ));
    }
    Ok(())
}
