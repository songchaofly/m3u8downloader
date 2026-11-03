use crate::{network::Network, playlist, AppError, Plan, Playlist, Result, TaskInput, Track};
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

pub async fn fetch_playlist(
    net: &Network,
    source: &str,
    base: Option<&str>,
    cancel: &CancellationToken,
) -> Result<Playlist> {
    let address = playlist::source_url(source)?.to_string();
    let r = net.get(&address, None, 10 * 1024 * 1024, cancel).await?;
    let text = std::str::from_utf8(&r.bytes)
        .map_err(|_| AppError::new("MANIFEST_INVALID", "播放列表必须使用 UTF-8"))?;
    let source = if address.starts_with("file:") {
        base.unwrap_or(&r.final_url)
    } else {
        &r.final_url
    };
    playlist::parse(text, source)
}

pub async fn build(net: &Network, input: &TaskInput, cancel: &CancellationToken) -> Result<Plan> {
    let mut current = fetch_playlist(net, &input.source, input.base_url.as_deref(), cancel).await?;
    let mut visited = HashSet::new();
    let mut total_text = current.raw.len();
    let mut resolution = "unknown".to_string();
    let mut bandwidth = 0;
    let mut tracks = Vec::new();
    for depth in 0..=5 {
        if !visited.insert(current.source.clone()) {
            return Err(AppError::new("MANIFEST_INVALID", "主列表循环引用"));
        }
        if !current.master {
            break;
        }
        if depth == 5 {
            return Err(AppError::new("LIMIT_EXCEEDED", "播放列表引用深度超过 5"));
        }
        let idx = if depth == 0 { input.variant } else { None }.unwrap_or_else(|| {
            current
                .variants
                .iter()
                .enumerate()
                .max_by_key(|(_, v)| v.bandwidth)
                .map(|(i, _)| i)
                .unwrap_or(0)
        });
        let selected = current
            .variants
            .get(idx)
            .ok_or_else(|| AppError::new("INPUT_INVALID", "选择的清晰度不存在"))?
            .clone();
        resolution = selected
            .resolution
            .clone()
            .unwrap_or_else(|| "unknown".into());
        bandwidth = selected.bandwidth;
        if !input.video_only {
            if let Some(group) = &selected.audio {
                let candidates = current
                    .renditions
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.kind == "AUDIO" && &r.group == group)
                    .collect::<Vec<_>>();
                let rendition = if let Some(i) = input.audio {
                    candidates
                        .iter()
                        .find(|(idx, _)| *idx == i)
                        .map(|(_, r)| *r)
                        .ok_or_else(|| AppError::new("INPUT_INVALID", "所选音轨不属于当前变体"))?
                } else {
                    candidates
                        .iter()
                        .find(|(_, r)| r.default)
                        .or(candidates.first())
                        .map(|(_, r)| *r)
                        .ok_or_else(|| AppError::new("MANIFEST_INVALID", "音轨组不存在"))?
                };
                if let Some(uri) = &rendition.uri {
                    let p = fetch_playlist(net, uri, None, cancel).await?;
                    total_text += p.raw.len();
                    tracks.push(Track {
                        kind: "audio".into(),
                        language: rendition.language.clone(),
                        playlist: p,
                    });
                }
            }
        }
        if let Some(i) = input.subtitle {
            let r = current
                .renditions
                .get(i)
                .filter(|r| r.kind == "SUBTITLES" && Some(&r.group) == selected.subtitles.as_ref())
                .ok_or_else(|| AppError::new("INPUT_INVALID", "所选字幕不属于当前变体"))?;
            let uri = r
                .uri
                .as_ref()
                .ok_or_else(|| AppError::new("HLS_UNSUPPORTED", "只支持外置 WebVTT 字幕"))?;
            let p = fetch_playlist(net, uri, None, cancel).await?;
            total_text += p.raw.len();
            tracks.push(Track {
                kind: "subtitle".into(),
                language: r.language.clone(),
                playlist: p,
            });
        }
        current = fetch_playlist(net, &selected.uri, None, cancel).await?;
        total_text += current.raw.len();
    }
    let duration = current.duration();
    tracks.insert(
        0,
        Track {
            kind: "video".into(),
            language: None,
            playlist: current,
        },
    );
    if total_text > 100 * 1024 * 1024
        || tracks
            .iter()
            .map(|t| t.playlist.segments.len())
            .sum::<usize>()
            > 100_000
    {
        return Err(AppError::new("LIMIT_EXCEEDED", "所选清单超过任务资源限制"));
    }
    for t in &tracks {
        if t.playlist.master || !t.playlist.ended {
            return Err(AppError::new(
                "HLS_UNSUPPORTED",
                "一期仅支持已结束的点播媒体列表",
            ));
        }
        if t.kind != "subtitle"
            && t.playlist.segments.iter().any(|s| {
                s.map.is_some()
                    || url::Url::parse(&s.uri).ok().is_some_and(|u| {
                        [".m4s", ".mp4", ".aac", ".mp3"]
                            .iter()
                            .any(|ext| u.path().to_lowercase().ends_with(ext))
                    })
            })
        {
            return Err(AppError::new(
                "HLS_UNSUPPORTED",
                "一期下载仅支持 MPEG-TS 媒体；fMP4 与 Packed Audio 尚未启用",
            ));
        }
    }
    Ok(Plan {
        tracks,
        resolution,
        bandwidth,
        duration,
    })
}
