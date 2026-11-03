use crate::{AppError, ByteRange, KeyContext, Playlist, Rendition, Result, Segment, Variant};
use std::collections::BTreeMap;
use url::Url;

fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("MANIFEST_INVALID", message)
}

pub fn source_url(source: &str) -> Result<Url> {
    if let Ok(u) = Url::parse(source) {
        if ["http", "https", "file"].contains(&u.scheme()) {
            return Ok(u);
        }
    }
    let path = std::path::Path::new(source);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    Url::from_file_path(path).map_err(|_| invalid("无法解析输入地址"))
}

pub fn resolve(base: &str, reference: &str) -> Result<String> {
    let base = source_url(base)?;
    let u = base
        .join(reference)
        .map_err(|_| invalid("无效的资源 URI"))?;
    if !["http", "https", "file"].contains(&u.scheme())
        || (base.scheme() != "file" && u.scheme() == "file")
    {
        return Err(AppError::new("HLS_UNSUPPORTED", "清单引用了不允许的协议"));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(invalid("请通过请求配置提供凭证，URL 不接受用户名或密码"));
    }
    Ok(u.to_string())
}

pub fn attributes(s: &str) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    let mut start = 0;
    let mut quoted = false;
    let mut fields = Vec::new();
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                fields.push(&s[start..i]);
                start = i + 1;
            }
            _ => (),
        }
    }
    if quoted {
        return Err(invalid("属性引号未闭合"));
    }
    fields.push(&s[start..]);
    for field in fields {
        let (k, v) = field.split_once('=').ok_or_else(|| invalid("无效属性"))?;
        if result
            .insert(k.trim().to_string(), v.trim().trim_matches('"').to_string())
            .is_some()
        {
            return Err(invalid("属性重复"));
        }
    }
    Ok(result)
}
fn required<'a>(a: &'a BTreeMap<String, String>, k: &str) -> Result<&'a str> {
    a.get(k)
        .map(String::as_str)
        .ok_or_else(|| invalid(format!("缺少属性 {k}")))
}
fn number<T: std::str::FromStr>(s: &str, label: &str) -> Result<T> {
    s.parse().map_err(|_| invalid(format!("无效的 {label}")))
}

pub fn parse(text: &str, source: &str) -> Result<Playlist> {
    if text.len() > 10 * 1024 * 1024 {
        return Err(AppError::new("LIMIT_EXCEEDED", "单个播放列表超过 10 MiB"));
    }
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.lines().next().map(str::trim) != Some("#EXTM3U") {
        return Err(invalid("响应不是 M3U8 播放列表，可能是登录页或错误正文"));
    }
    let source = source_url(source)?.to_string();
    let mut list = Playlist {
        source: source.clone(),
        master: false,
        ended: false,
        target_duration: 0.0,
        segments: Vec::new(),
        variants: Vec::new(),
        renditions: Vec::new(),
        warnings: Vec::new(),
        raw: text.into(),
    };
    let mut duration: Option<f64> = None;
    let mut variant = None;
    let mut sequence: u64 = 0;
    let mut disc: u64 = 0;
    let mut key: Option<KeyContext> = None;
    let mut key_id: usize = 0;
    let mut pending_range: Option<(u64, Option<u64>)> = None;
    let mut previous_range: Option<(String, u64)> = None;
    let mut map = None;
    for (line_no, line) in text.lines().enumerate().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with('#') {
            let uri = resolve(&source, line)?;
            if let Some(a) = variant.take() {
                let a: BTreeMap<String, String> = a;
                list.variants.push(Variant {
                    uri,
                    bandwidth: number(required(&a, "BANDWIDTH")?, "带宽")?,
                    resolution: a.get("RESOLUTION").cloned(),
                    codecs: a.get("CODECS").cloned(),
                    audio: a.get("AUDIO").cloned(),
                    subtitles: a.get("SUBTITLES").cloned(),
                });
                list.master = true;
            } else {
                let d = duration
                    .take()
                    .ok_or_else(|| invalid(format!("第 {} 行缺少 EXTINF", line_no + 1)))?;
                if !d.is_finite() || d <= 0.0 {
                    return Err(invalid("分片时长必须为正数"));
                }
                let range = if let Some((length, offset)) = pending_range.take() {
                    let offset = match offset {
                        Some(v) => v,
                        None => previous_range
                            .as_ref()
                            .filter(|(u, _)| u == &uri)
                            .map(|(_, n)| *n)
                            .ok_or_else(|| invalid("隐含字节偏移要求前一分片引用同一资源"))?,
                    };
                    previous_range = Some((
                        uri.clone(),
                        offset
                            .checked_add(length)
                            .ok_or_else(|| invalid("范围溢出"))?,
                    ));
                    Some(ByteRange { offset, length })
                } else {
                    previous_range = None;
                    None
                };
                list.segments.push(Segment {
                    index: list.segments.len(),
                    sequence,
                    uri,
                    duration: d,
                    range,
                    discontinuity: disc,
                    key: key.clone(),
                    map: map.clone(),
                });
                sequence = sequence
                    .checked_add(1)
                    .ok_or_else(|| invalid("媒体序号溢出"))?;
                if list.segments.len() > 100_000 {
                    return Err(AppError::new("LIMIT_EXCEEDED", "分片数超过上限"));
                }
            }
            continue;
        }
        let (tag, value) = line.split_once(':').unwrap_or((line, ""));
        match tag {
            "#EXTINF" => {
                if duration.is_some() {
                    return Err(invalid("EXTINF 后缺少资源地址"));
                }
                duration = Some(number(value.split(',').next().unwrap_or(""), "时长")?);
            }
            "#EXT-X-TARGETDURATION" => {
                list.target_duration = number(value, "目标时长")?;
            }
            "#EXT-X-MEDIA-SEQUENCE" => {
                if !list.segments.is_empty() {
                    return Err(invalid("MEDIA-SEQUENCE 必须位于分片之前"));
                }
                sequence = number(value, "媒体序号")?;
            }
            "#EXT-X-DISCONTINUITY-SEQUENCE" => disc = number(value, "不连续序号")?,
            "#EXT-X-DISCONTINUITY" => {
                disc = disc
                    .checked_add(1)
                    .ok_or_else(|| invalid("不连续序号溢出"))?
            }
            "#EXT-X-ENDLIST" => list.ended = true,
            "#EXT-X-STREAM-INF" => {
                if variant.is_some() {
                    return Err(invalid("变体缺少地址"));
                }
                variant = Some(attributes(value)?);
            }
            "#EXT-X-MEDIA" => {
                let a = attributes(value)?;
                list.renditions.push(Rendition {
                    kind: required(&a, "TYPE")?.into(),
                    group: required(&a, "GROUP-ID")?.into(),
                    name: required(&a, "NAME")?.into(),
                    language: a.get("LANGUAGE").cloned(),
                    uri: a.get("URI").map(|v| resolve(&source, v)).transpose()?,
                    default: a.get("DEFAULT").is_some_and(|v| v == "YES"),
                });
            }
            "#EXT-X-KEY" => {
                let a = attributes(value)?;
                key_id += 1;
                key = match required(&a, "METHOD")? {
                    "NONE" => None,
                    "AES-128" if a.get("KEYFORMAT").is_none_or(|v| v == "identity") => {
                        let iv = a.get("IV").cloned();
                        if let Some(v) = &iv {
                            let h = v.trim_start_matches("0x").trim_start_matches("0X");
                            if h.is_empty()
                                || h.len() > 32
                                || !h.chars().all(|c| c.is_ascii_hexdigit())
                            {
                                return Err(invalid("无效的 128 位 IV"));
                            }
                        }
                        Some(KeyContext {
                            id: key_id,
                            uri: resolve(&source, required(&a, "URI")?)?,
                            iv,
                        })
                    }
                    _ => {
                        return Err(AppError::new(
                            "ENCRYPTION_UNSUPPORTED",
                            "只支持 identity AES-128；不支持 SAMPLE-AES 或 DRM",
                        ))
                    }
                };
            }
            "#EXT-X-BYTERANGE" => {
                let (n, off) = value
                    .split_once('@')
                    .map(|(n, o)| (n, Some(o)))
                    .unwrap_or((value, None));
                let n: u64 = number(n, "字节长度")?;
                if n == 0 {
                    return Err(invalid("字节长度不能为 0"));
                }
                pending_range = Some((n, off.map(|v| number(v, "字节偏移")).transpose()?));
            }
            "#EXT-X-MAP" => {
                let a = attributes(value)?;
                if a.contains_key("BYTERANGE") {
                    return Err(AppError::new(
                        "HLS_UNSUPPORTED",
                        "一期不支持带范围的初始化段",
                    ));
                }
                map = Some(resolve(&source, required(&a, "URI")?)?);
            }
            "#EXT-X-VERSION" => {
                let _: u32 = number(value, "列表版本")?;
            }
            "#EXT-X-PLAYLIST-TYPE"
            | "#EXT-X-INDEPENDENT-SEGMENTS"
            | "#EXT-X-PROGRAM-DATE-TIME"
            | "#EXT-X-DATERANGE"
            | "#EXT-X-START"
            | "#EXT-X-ALLOW-CACHE" => {}
            "#EXT-X-I-FRAME-STREAM-INF" => {
                list.warnings.push("未选择 I-frame 专用流".into());
            }
            _ if tag.starts_with("#EXT") => {
                return Err(AppError::new(
                    "HLS_UNSUPPORTED",
                    format!("尚未支持的标签 {tag}（第 {} 行）", line_no + 1),
                ))
            }
            _ => {}
        }
    }
    if duration.is_some() || variant.is_some() || pending_range.is_some() {
        return Err(invalid("列表末尾缺少资源地址"));
    }
    if list.master && !list.segments.is_empty() {
        return Err(invalid("主列表与媒体列表标签混用"));
    }
    if !list.master
        && (list.segments.is_empty()
            || !list.target_duration.is_finite()
            || list.target_duration <= 0.0)
    {
        return Err(invalid("媒体列表为空或缺少有效 TARGETDURATION"));
    }
    Ok(list)
}
