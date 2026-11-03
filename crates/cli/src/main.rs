use clap::{Args, Parser, Subcommand};
use m3u8_core::{AppError, Engine, Result, TaskInput};
use std::{path::PathBuf, sync::Arc};

#[derive(Parser)]
#[command(
    name = "m3u8tool",
    version,
    about = "M3U8 Studio · 本地 HLS 下载与媒体处理"
)]
struct Cli {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    Parse {
        #[arg(long)]
        url: String,
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(long)]
        request_config: Option<PathBuf>,
    },
    Download(Download),
    List,
    Capabilities,
}
#[derive(Args)]
struct Download {
    #[arg(long)]
    url: Option<String>,
    #[arg(long)]
    resume: Option<String>,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    request_config: Option<PathBuf>,
    #[arg(long)]
    key_file: Option<PathBuf>,
    #[arg(long)]
    key: Option<String>,
    #[arg(long)]
    iv: Option<String>,
    #[arg(long)]
    cookie: Option<String>,
    #[arg(long)]
    referer: Option<String>,
    #[arg(long)]
    user_agent: Option<String>,
    #[arg(long = "header")]
    headers: Vec<String>,
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long)]
    concurrency: Option<usize>,
    #[arg(long)]
    variant: Option<usize>,
    #[arg(long)]
    audio: Option<usize>,
    #[arg(long)]
    subtitle: Option<usize>,
    #[arg(long)]
    video_only: bool,
    #[arg(long, default_value = "copy")]
    video_codec: String,
    #[arg(long, default_value = "copy")]
    audio_codec: String,
    #[arg(long, default_value = "sidecar")]
    subtitle_mode: String,
    #[arg(long)]
    keep_cache: bool,
    #[arg(long)]
    remember: bool,
    #[arg(long)]
    allow_gaps: bool,
    #[arg(long)]
    fast_ts: bool,
    #[arg(long)]
    local_root: Option<String>,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, default_value = "jsonl")]
    format: String,
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg: String,
    #[arg(long, default_value = "ffprobe")]
    ffprobe: String,
    #[arg(long)]
    low_disk_mib: Option<u64>,
}
fn data_dir() -> PathBuf {
    if let Some(s) = std::env::var_os("M3U8_STUDIO_DATA_DIR") {
        return s.into();
    }
    if let Some(s) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(s).join("M3U8Studio");
    }
    if let Some(s) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(s).join("m3u8studio");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/m3u8studio")
}
fn exit_code(error: &AppError) -> i32 {
    if error.code == "CANCELLED" {
        130
    } else if error.code.starts_with("KEY")
        || error.code.starts_with("DECRYPT")
        || error.code.starts_with("ENCRYPTION")
    {
        4
    } else if error.code.starts_with("HTTP")
        || error.code.starts_with("NETWORK")
        || error.code.starts_with("PROXY")
        || error.code == "CREDENTIAL_REQUIRED"
    {
        3
    } else if error.code.starts_with("OUTPUT") || error.code.starts_with("FFMPEG") {
        6
    } else if error.code.starts_with("STORE")
        || error.code.starts_with("IO")
        || error.code.starts_with("DISK")
        || error.code.starts_with("PERMISSION")
    {
        5
    } else {
        2
    }
}
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{}", serde_json::to_string(&e).unwrap());
            std::process::exit(exit_code(&e));
        }
    }
}

async fn run(cli: Cli) -> Result<i32> {
    let engine = Engine::open(&cli.data_dir.unwrap_or_else(data_dir))?;
    match cli.command {
        Commands::List => {
            println!("{}", serde_json::to_string_pretty(&engine.list())?);
            Ok(0)
        }
        Commands::Capabilities => {
            println!(
                "{}",
                serde_json::to_string_pretty(&engine.diagnostics().await)?
            );
            Ok(0)
        }
        Commands::Parse {
            url,
            format,
            request_config,
        } => {
            if format != "json" {
                return Err(AppError::new("INPUT_INVALID", "parse 仅支持 --format json"));
            }
            let mut input = TaskInput {
                source: url,
                ..Default::default()
            };
            if let Some(p) = request_config {
                input.network = serde_json::from_slice(&std::fs::read(p)?)?;
            }
            let p = engine.inspect(&input).await?;
            // Explicit parse output includes source URIs; diagnostics never do.
            println!("{}", serde_json::to_string_pretty(&p)?);
            Ok(0)
        }
        Commands::Download(d) => download(engine, d).await,
    }
}
async fn download(engine: Arc<Engine>, d: Download) -> Result<i32> {
    if d.format != "jsonl" {
        return Err(AppError::new(
            "INPUT_INVALID",
            "download 仅支持 --format jsonl",
        ));
    }
    if let Some(n) = d.low_disk_mib {
        let mut s = engine.settings();
        s.low_disk_mib = n;
        engine.save_settings(s)?;
    }
    let mut input = if let Some(id) = &d.resume {
        engine.input(id)?.unwrap_or_default()
    } else {
        TaskInput::default()
    };
    if let Some(url) = d.url {
        input.source = url;
    }
    if input.source.is_empty() {
        return Err(AppError::new(
            "INPUT_INVALID",
            "需要 --url 或可恢复的 --resume 任务",
        ));
    }
    if let Some(output) = d.output {
        let absolute = if output.is_absolute() {
            output
        } else {
            std::env::current_dir()?.join(output)
        };
        if let Some(ext) = absolute.extension().and_then(|s| s.to_str()) {
            input.container = ext.to_lowercase();
            input.output_dir = absolute.parent().unwrap().to_string_lossy().into_owned();
            input.filename_template = absolute.file_stem().unwrap().to_string_lossy().into_owned();
        } else {
            input.output_dir = absolute.to_string_lossy().into_owned();
        }
    }
    if let Some(title) = d.title {
        input.title = title;
    }
    if let Some(p) = d.request_config {
        input.network = serde_json::from_slice(&std::fs::read(p)?)?;
    }
    for (key, value) in [("Cookie", d.cookie), ("Referer", d.referer)] {
        if let Some(v) = value {
            input.network.headers.insert(key.into(), v);
        }
    }
    for header in d.headers {
        let (k, v) = header
            .split_once(':')
            .ok_or_else(|| AppError::new("INPUT_INVALID", "--header 使用 Name: value 格式"))?;
        input
            .network
            .headers
            .insert(k.trim().into(), v.trim().into());
    }
    if let Some(v) = d.user_agent {
        input.network.user_agent = v;
    }
    if let Some(v) = d.proxy {
        input.network.proxy = v;
    }
    if let Some(v) = d.concurrency {
        input.network.concurrency = v;
    }
    let key = if let Some(p) = d.key_file {
        let bytes = std::fs::read(p)?;
        Some(if bytes.len() == 16 {
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        } else {
            String::from_utf8(bytes)
                .map_err(|_| AppError::new("KEY_INVALID", "密钥文件必须为 16 字节或文本"))?
        })
    } else {
        d.key
    };
    if let Some(key) = key {
        input.manual_keys.insert("1".into(), key);
    }
    if let Some(iv) = d.iv {
        input.manual_ivs.insert("1".into(), iv);
    }
    input.variant = d.variant.or(input.variant);
    input.audio = d.audio.or(input.audio);
    input.subtitle = d.subtitle.or(input.subtitle);
    input.video_only = d.video_only;
    input.video_codec = d.video_codec;
    input.audio_codec = d.audio_codec;
    input.subtitle_mode = d.subtitle_mode;
    input.keep_cache = d.keep_cache;
    input.remember = d.remember;
    input.allow_gaps = d.allow_gaps;
    input.fast_ts = d.fast_ts;
    input.local_root = d.local_root.or(input.local_root);
    input.base_url = d.base_url.or(input.base_url);
    input.ffmpeg = d.ffmpeg;
    input.ffprobe = d.ffprobe;
    let id = if let Some(id) = d.resume {
        engine.update_input(&id, input)?;
        id
    } else {
        engine.add(input)?.id
    };
    let mut events = engine.subscribe();
    engine.start(&id)?;
    let mut interrupt = Box::pin(tokio::signal::ctrl_c());
    loop {
        tokio::select! {
            _=&mut interrupt=>{engine.stop(&id,false).await?;return Ok(130);}
            event=events.recv()=>{
                let view=match event{Ok(v)=>v,Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>continue,Err(_)=>return Err(AppError::new("INTERNAL","任务事件通道关闭"))};
                if view.id!=id{continue;}
                let event=match view.state.as_str(){"completed"=>"completed","completed_with_gaps"=>"completed_with_gaps","failed"|"waiting"=>"failed","cancelled"=>"cancelled",_=>"progress"};
                println!("{}",serde_json::to_string(&serde_json::json!({"version":1,"event":event,"task":view}))?);
                match view.state.as_str(){"completed"=>return Ok(0),"completed_with_gaps"=>return Ok(7),"failed"|"waiting"=>return Err(view.error.unwrap_or_else(||AppError::new("INTERNAL","任务未完成"))),"cancelled"=>return Ok(130),_=>{}}
            }
        }
    }
}
