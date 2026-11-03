use crate::{AppError, ByteRange, NetworkConfig, Result};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client, StatusCode,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{Mutex, Semaphore},
    time::{sleep, timeout, Instant},
};
use tokio_util::sync::CancellationToken;
use url::Url;

pub struct Resource {
    pub final_url: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone)]
pub struct Network {
    client: Client,
    config: NetworkConfig,
    origin: String,
    local_root: Option<PathBuf>,
    global: Arc<Semaphore>,
    local: Arc<Semaphore>,
    limiter: Arc<Mutex<Instant>>,
    pub received: Arc<AtomicU64>,
}
fn request_error(e: reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::new("NETWORK_TIMEOUT", "网络请求超时").retryable()
    } else if e.is_connect() {
        AppError::new(
            "NETWORK_CONNECT",
            "无法建立连接，请检查代理、网络或 TLS 证书",
        )
        .retryable()
    } else {
        AppError::new("NETWORK_FAILED", "网络响应异常或传输中断").retryable()
    }
}
pub fn origin(u: &Url) -> String {
    u.origin().ascii_serialization()
}
pub fn display_url(s: &str) -> String {
    if let Ok(u) = crate::playlist::source_url(s) {
        if u.scheme() == "file" {
            return "本地播放列表".into();
        }
        return format!("{} / …", origin(&u));
    }
    "输入来源".into()
}

fn headers(config: &NetworkConfig, source_origin: &str, target: &Url) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    map.insert(
        "user-agent",
        HeaderValue::from_str(&config.user_agent)
            .map_err(|_| AppError::new("INPUT_INVALID", "User-Agent 含无效字符"))?,
    );
    map.insert("accept-encoding", HeaderValue::from_static("identity"));
    let target_origin = origin(target);
    let values = if target_origin == source_origin {
        Some(&config.headers)
    } else {
        config.origin_headers.get(&target_origin)
    };
    if let Some(values) = values {
        let mut seen = std::collections::HashSet::new();
        for (k, v) in values {
            let key = k.to_ascii_lowercase();
            if [
                "host",
                "content-length",
                "connection",
                "transfer-encoding",
                "proxy-authorization",
                "accept-encoding",
                "range",
            ]
            .contains(&key.as_str())
                || !seen.insert(key.clone())
            {
                return Err(AppError::new(
                    "INPUT_INVALID",
                    "请求头重复或包含由网络栈管理的字段",
                ));
            }
            if v.contains(['\r', '\n']) {
                return Err(AppError::new("INPUT_INVALID", "请求头不可包含换行"));
            }
            map.insert(
                HeaderName::from_bytes(key.as_bytes())
                    .map_err(|_| AppError::new("INPUT_INVALID", "无效请求头名称"))?,
                HeaderValue::from_str(v)
                    .map_err(|_| AppError::new("INPUT_INVALID", "无效请求头值"))?,
            );
        }
    }
    Ok(map)
}

impl Network {
    pub fn new(
        config: NetworkConfig,
        source: &str,
        root: Option<&str>,
        global: Arc<Semaphore>,
    ) -> Result<Self> {
        if !(1..=32).contains(&config.concurrency)
            || config.retries > 10
            || !(1..=120).contains(&config.connect_timeout)
            || !(1..=3600).contains(&config.request_timeout)
            || !(5..=300).contains(&config.read_timeout)
            || (config.limit_kib != 0 && !(64..=1_048_576).contains(&config.limit_kib))
        {
            return Err(AppError::new("INPUT_INVALID", "网络参数超出允许范围"));
        }
        let u = crate::playlist::source_url(source)?;
        let local_root = if u.scheme() == "file" {
            let file = u
                .to_file_path()
                .map_err(|_| AppError::new("INPUT_INVALID", "本地文件路径无效"))?;
            Some(std::fs::canonicalize(
                root.map(PathBuf::from)
                    .unwrap_or_else(|| file.parent().unwrap_or(Path::new(".")).to_owned()),
            )?)
        } else {
            None
        };
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(config.connect_timeout))
            .min_tls_version(reqwest::tls::Version::TLS_1_2);
        match config.proxy.as_str() {
            "" | "direct" => builder = builder.no_proxy(),
            "system" => {}
            proxy => {
                let u =
                    Url::parse(proxy).map_err(|_| AppError::new("PROXY_FAILED", "代理地址无效"))?;
                if !["http", "https", "socks5", "socks5h"].contains(&u.scheme()) {
                    return Err(AppError::new("PROXY_FAILED", "代理协议不支持"));
                }
                builder = builder.no_proxy().proxy(
                    reqwest::Proxy::all(proxy)
                        .map_err(|_| AppError::new("PROXY_FAILED", "代理配置无效"))?,
                );
            }
        }
        headers(&config, &origin(&u), &u)?;
        for target in config.origin_headers.keys() {
            let target = Url::parse(target)
                .map_err(|_| AppError::new("INPUT_INVALID", "凭证作用域必须是有效的源"))?;
            headers(&config, &origin(&u), &target)?;
        }
        Ok(Self {
            client: builder.build().map_err(request_error)?,
            origin: origin(&u),
            local_root,
            global,
            local: Arc::new(Semaphore::new(config.concurrency)),
            limiter: Arc::new(Mutex::new(Instant::now())),
            received: Arc::new(AtomicU64::new(0)),
            config,
        })
    }

    pub async fn get(
        &self,
        address: &str,
        range: Option<&ByteRange>,
        max_bytes: usize,
        cancel: &CancellationToken,
    ) -> Result<Resource> {
        let u = Url::parse(address).map_err(|_| AppError::new("INPUT_INVALID", "资源地址无效"))?;
        if u.scheme() == "file" {
            return self.local_file(&u, range, max_bytes, cancel).await;
        }
        if !["http", "https"].contains(&u.scheme())
            || !u.username().is_empty()
            || u.password().is_some()
        {
            return Err(AppError::new(
                "INPUT_INVALID",
                "禁止此资源协议或 URL 中的凭证",
            ));
        }
        for attempt in 0..=self.config.retries {
            let result = tokio::select! {
                _=cancel.cancelled()=>return Err(AppError::cancelled()),
                result=async {
                    let _local=self.local.acquire().await.map_err(|_|AppError::cancelled())?;
                    let _global=self.global.acquire().await.map_err(|_|AppError::cancelled())?;
                    timeout(Duration::from_secs(self.config.request_timeout),self.remote_once(u.clone(),range,max_bytes,cancel)).await.map_err(|_|AppError::new("NETWORK_TIMEOUT","请求超过总时限").retryable())?
                }=>result,
            };
            match result {
                Ok(r) => return Ok(r),
                Err(e) if e.retryable && attempt < self.config.retries => {
                    let wait = (2u64.pow(attempt.min(5)))
                        .min(30)
                        .max(e.retry_after_secs.unwrap_or(0));
                    let jitter = rand::random::<u16>() as u64 % 250;
                    tokio::select! { _=cancel.cancelled()=>return Err(AppError::cancelled()), _=sleep(Duration::from_millis(wait.saturating_mul(1000).saturating_add(jitter)))=>{} }
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    async fn remote_once(
        &self,
        mut u: Url,
        range: Option<&ByteRange>,
        max: usize,
        cancel: &CancellationToken,
    ) -> Result<Resource> {
        let mut visited = std::collections::HashSet::new();
        for hop in 0..=5 {
            if !visited.insert(u.to_string()) {
                return Err(AppError::new("NETWORK_REDIRECT", "重定向循环"));
            }
            let mut req =
                self.client
                    .get(u.clone())
                    .headers(headers(&self.config, &self.origin, &u)?);
            if let Some(r) = range {
                let end = r
                    .offset
                    .checked_add(r.length)
                    .and_then(|n| n.checked_sub(1))
                    .ok_or_else(|| AppError::new("RANGE_INVALID", "范围溢出"))?;
                req = req.header("range", format!("bytes={}-{}", r.offset, end));
            }
            let resp = req.send().await.map_err(request_error)?;
            if resp.status().is_redirection() {
                if hop == 5 {
                    return Err(AppError::new("NETWORK_REDIRECT", "重定向次数超过 5 次"));
                }
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| AppError::new("NETWORK_REDIRECT", "重定向缺少 Location"))?;
                let next = u
                    .join(location)
                    .map_err(|_| AppError::new("NETWORK_REDIRECT", "重定向目标无效"))?;
                if !["http", "https"].contains(&next.scheme())
                    || (u.scheme() == "https" && next.scheme() == "http")
                    || !next.username().is_empty()
                    || next.password().is_some()
                {
                    return Err(AppError::new("NETWORK_REDIRECT", "拒绝不安全的重定向目标"));
                }
                u = next;
                continue;
            }
            let status = resp.status();
            if !status.is_success() {
                let code = match status.as_u16() {
                    401 | 403 => "HTTP_AUTH",
                    404 => "HTTP_NOT_FOUND",
                    429 => "HTTP_RATE_LIMIT",
                    _ => "HTTP_ERROR",
                };
                let mut e = AppError::new(code, format!("服务器返回 HTTP {}", status.as_u16()));
                e.retryable = matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504);
                e.retry_after_secs = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| {
                        v.parse().ok().or_else(|| {
                            httpdate::parse_http_date(v).ok().map(|t| {
                                t.duration_since(SystemTime::now())
                                    .unwrap_or_default()
                                    .as_secs()
                            })
                        })
                    });
                return Err(e);
            }
            if let Some(enc) = resp.headers().get("content-encoding") {
                if enc != "identity" {
                    return Err(AppError::new(
                        "SEGMENT_INVALID",
                        "服务器返回了不支持的内容编码",
                    ));
                }
            }
            if let Some(r) = range {
                let expected = format!("bytes {}-{}/", r.offset, r.offset + r.length - 1);
                if status != StatusCode::PARTIAL_CONTENT
                    || !resp
                        .headers()
                        .get("content-range")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v.starts_with(&expected))
                {
                    return Err(AppError::new("RANGE_INVALID", "服务器未返回请求的字节范围"));
                }
            }
            let expected = resp.content_length();
            if expected.is_some_and(|v| v > max as u64) {
                return Err(AppError::new(
                    "LIMIT_EXCEEDED",
                    "响应超过本次请求的资源上限",
                ));
            }
            let mut data = Vec::with_capacity(expected.unwrap_or(4096).min(max as u64) as usize);
            let mut stream = resp.bytes_stream();
            while let Some(chunk) =
                timeout(Duration::from_secs(self.config.read_timeout), stream.next())
                    .await
                    .map_err(|_| AppError::new("NETWORK_TIMEOUT", "读取空闲超时").retryable())?
            {
                let chunk = chunk.map_err(request_error)?;
                if data.len() + chunk.len() > max {
                    return Err(AppError::new(
                        "LIMIT_EXCEEDED",
                        "响应超过本次请求的资源上限",
                    ));
                }
                self.received
                    .fetch_add(chunk.len() as u64, Ordering::Relaxed);
                if self.config.limit_kib > 0 {
                    let mut next = self.limiter.lock().await;
                    let start = (*next).max(Instant::now());
                    *next = start
                        + Duration::from_secs_f64(
                            chunk.len() as f64 / (self.config.limit_kib as f64 * 1024.0),
                        );
                    let until = *next;
                    drop(next);
                    tokio::select! {_=cancel.cancelled()=>return Err(AppError::cancelled()),_=tokio::time::sleep_until(until)=>{}}
                }
                data.extend_from_slice(&chunk);
            }
            if expected.is_some_and(|n| n != data.len() as u64)
                || range.is_some_and(|r| r.length != data.len() as u64)
            {
                return Err(AppError::new("SEGMENT_INVALID", "传输长度与预期不一致").retryable());
            }
            if data.is_empty() {
                return Err(AppError::new("SEGMENT_INVALID", "服务器返回空内容"));
            }
            return Ok(Resource {
                final_url: u.to_string(),
                bytes: data,
            });
        }
        unreachable!()
    }
    async fn local_file(
        &self,
        u: &Url,
        range: Option<&ByteRange>,
        max: usize,
        cancel: &CancellationToken,
    ) -> Result<Resource> {
        if cancel.is_cancelled() {
            return Err(AppError::cancelled());
        }
        let root = self
            .local_root
            .as_ref()
            .ok_or_else(|| AppError::new("PERMISSION_DENIED", "网络清单不能读取本地文件"))?;
        if u.host_str()
            .is_some_and(|h| h != "localhost" && !h.is_empty())
        {
            return Err(AppError::new("PERMISSION_DENIED", "不允许 UNC 输入"));
        }
        let path = tokio::fs::canonicalize(
            u.to_file_path()
                .map_err(|_| AppError::new("INPUT_INVALID", "文件路径无效"))?,
        )
        .await?;
        if !path.starts_with(root) {
            return Err(AppError::new("PERMISSION_DENIED", "资源超出已授权目录"));
        }
        let mut file = tokio::fs::File::open(path).await?;
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let size = file.metadata().await?.len();
        let length = if let Some(r) = range {
            if r.offset.checked_add(r.length).is_none_or(|v| v > size) {
                return Err(AppError::new("RANGE_INVALID", "本地范围超出文件"));
            }
            file.seek(std::io::SeekFrom::Start(r.offset)).await?;
            r.length
        } else {
            size
        };
        if length > max as u64 {
            return Err(AppError::new("LIMIT_EXCEEDED", "本地资源超过读取上限"));
        }
        let mut bytes = vec![0; length as usize];
        file.read_exact(&mut bytes).await?;
        Ok(Resource {
            final_url: u.to_string(),
            bytes,
        })
    }
}
