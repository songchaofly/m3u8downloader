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

fn command(exe: &str) -> Command {
    let mut c = Command::new(exe);
    c.kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(0x08000000);
    c
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
    let probe = command(ffprobe).arg("-version").output().await?;
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
) -> Result<OutputResult> {
    validate_input(input)?;
    let output_dir = PathBuf::from(&input.output_dir);
    tokio::fs::create_dir_all(&output_dir).await?;
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
        let lock = output.with_extension(format!("{}.reserve", input.container));
        if !output.exists() {
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
    struct Reservation(PathBuf);
    impl Drop for Reservation {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _reservation = Reservation(reservation);
    let temporary = output_dir.join(format!(".{id}.part.{}", input.container));
    if temporary.exists() {
        tokio::fs::remove_file(&temporary).await?;
    }
    let mut inputs = Vec::new();
    let mut subtitle = None;
    for (ti, track) in plan.tracks.iter().enumerate() {
        if track.kind == "subtitle" {
            let mut parts = Vec::new();
            let mut elapsed = 0.0;
            for segment in &track.playlist.segments {
                let file = cache.join(format!("{ti}-{}.vtt", segment.index));
                if file.exists() {
                    parts.push((tokio::fs::read_to_string(file).await?, elapsed));
                }
                elapsed += segment.duration;
            }
            let text = merge_vtt(&parts)?;
            let path = cache.join("subtitles.vtt");
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
        tokio::fs::write(&path, text).await?;
        inputs.push((track.kind.clone(), path));
    }
    if input.fast_ts {
        if input.container != "ts"
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
                "快速 TS 拼接要求单一轨道组、无断点且不重新编码",
            ));
        }
        let mut out = tokio::fs::File::create(&temporary).await?;
        for segment in &plan.tracks[0].playlist.segments {
            if cancel.is_cancelled() {
                return Err(AppError::cancelled());
            }
            if missing.contains(&format!("0:{}", segment.index)) {
                continue;
            }
            let mut f =
                tokio::fs::File::open(cache.join(format!("0-{}.ts", segment.index))).await?;
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
                p.to_string_lossy().into_owned(),
            ]);
        }
        let embed = subtitle.is_some() && input.subtitle_mode == "embed" && input.container != "ts";
        if embed {
            args.extend([
                "-protocol_whitelist".into(),
                "file".into(),
                "-i".into(),
                subtitle.as_ref().unwrap().to_string_lossy().into_owned(),
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
            temporary.to_string_lossy().into_owned(),
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
        temporary.to_string_lossy().into_owned(),
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
    if let Some(sub) = subtitle {
        if input.subtitle_mode == "sidecar" || input.container == "ts" {
            let dest = output.with_extension("vtt");
            if dest.exists() {
                return Err(AppError::new("OUTPUT_CONFLICT", "旁置字幕已存在，未覆盖"));
            }
            tokio::fs::copy(sub, &dest).await?;
            result.subtitle = Some(dest.to_string_lossy().into_owned());
        }
    }
    if !missing.is_empty() {
        let report = output.with_extension("missing.json");
        tokio::fs::write(
            report,
            serde_json::to_vec_pretty(&serde_json::json!({"task_id":id,"missing":missing}))?,
        )
        .await?;
    }
    tokio::fs::rename(&temporary, &output).await?;
    result.path = output.to_string_lossy().into_owned();
    progress(1.0);
    Ok(result)
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
    let _diagnostics = errors.await;
    if cancel.is_cancelled() {
        return Err(AppError::cancelled());
    }
    if !status.success() {
        return Err(AppError::new(
            "OUTPUT_INCOMPATIBLE",
            "FFmpeg 处理失败，请检查编码与容器兼容性；缓存已保留",
        ));
    }
    Ok(())
}
