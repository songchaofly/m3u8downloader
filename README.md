# M3U8 Studio

本机运行的 HLS 点播下载和媒体处理工作台。包含 Tauri 桌面端、Rust 核心引擎及复用同一引擎的 CLI。当前实测目标为 Windows 11 x64；其他平台尚未做安装和运行验收。

当前为 0.1.0 本地验收版：核心工作流、安装版和便携版已实际验证，40 项集成测试通过；内存与吞吐性能门槛仍未全部满足。完整结果和未关闭项见 [验收报告](docs/ACCEPTANCE.md)，不将本版标为全部 P0 验收完成。

## 使用

安装版运行 `release` 目录中的 NSIS 安装程序。便携版解压后运行 `Launch.cmd`，数据保存在便携目录的 `data` 中；直接运行 `m3u8-studio.exe` 时使用系统应用数据目录。程序随附 FFmpeg/ffprobe，不需要另行安装媒体工具。便携运行要求系统已有 WebView2；安装包携带 WebView2 离线安装器。

1. 点击“新建任务”，粘贴 URL 或选择本地 M3U8。
2. 点击“解析”，确认清晰度、音轨、字幕和输出目录。
3. 如来源需要授权，在高级选项设置 Cookie、Referer、Authorization 或按源绑定的请求头。不同源的凭证独立配置。
4. 点击“开始下载”。暂停后可继续，崩溃或重启后任务先保持暂停。
5. 完成后播放输出或打开目录。后处理失败可修改编码/容器/工具路径再继续；完整、有效的缓存可离线重试后处理。

默认拒绝覆盖现有输出，自动增加序号。“允许覆盖”必须主动勾选，验证新结果后才替换。允许跳过媒体分片时，会先展示缺失范围，确认后生成 `.incomplete` 文件和缺失报告；选择的字幕、密钥、初始化信息失败不能跳过。

## 能力与边界

| 功能 | 本版行为 |
|---|---|
| 输入 | HTTP/HTTPS、本地 M3U8、主列表、相对路径、批量导入、显式开启的剪贴板提示 |
| HLS 媒体 | 已结束的 MPEG-TS 点播；AES-128 CBC、媒体序号/显式 IV、密钥轮换、METHOD=NONE、BYTERANGE、DISCONTINUITY |
| 音轨/字幕 | 一条选定的独立 AAC TS 音轨；WebVTT 旁置文件或 MP4/MKV 软字幕；TS 使用旁置字幕 |
| 输出 | MP4/MKV/TS，copy、H.264、H.265、AAC；快速 TS 要求每片流结构一致 |
| 预览 | 经 Rust IPC 请求的 HLS 预览；Cookie、代理和密钥规则与下载相同；本地 MP4、经临时转封装的 TS/MKV、VTT |
| 网络 | 直连、系统、HTTP、SOCKS5/SOCKS5h 代理；超时、有限重试、Retry-After、任务限速、统一请求并发 |
| 任务 | 多任务、暂停/继续/取消、缓存校验恢复、日志、分片与密钥上下文详情、执行历史 |
| 界面 | 简体中文/英文、浅色/深色/跟随系统、键盘焦点、可关闭的完成通知 |
| 未纳入 | 直播录制、fMP4、DRM/SAMPLE-AES、同时保留多音轨、字节级续传、自动更新、远程控制 |

内嵌播放器可用编码受系统 WebView2 媒体能力影响；下载转码支持与内嵌播放能力分开检测。H.265 输出可用，但不能保证每台设备的 WebView2 都可直接播放 H.265。

## 数据与凭证

- 安装版默认目录：`%LOCALAPPDATA%\io.m3u8studio.desktop`。CLI 默认目录：`%LOCALAPPDATA%\M3U8Studio`。可用 `M3U8_STUDIO_DATA_DIR` 或 CLI `--data-dir` 指定隔离目录。
- `tasks.sqlite` 保存任务、执行和分片索引；敏感任务快照使用 AES-GCM 保护，包装密钥位于操作系统凭证存储。
- 不勾选“记住”时，Header、代理口令、手动密钥不跨重启保存；加密保护的来源清单可保留。安全存储不可用时保留非敏感记录，恢复时要求补充配置，不能降级为明文。
- 全局按源绑定的请求头预设默认只在本次运行有效；创建任务时复制快照，可对该任务单独选择安全记住。
- 日志位于 `logs`；支持 DEBUG/INFO/WARN/ERROR，默认 7 天、总量 100 MiB、单文件 10 MiB。日志不记录请求凭证或完整签名 URL。
- 完整成功后默认清理分片；失败、暂停、取消、有缺失任务保留缓存。删除记录/缓存不会删除正式输出。

## 开发与验证

本次环境：Windows 11 x64、Rust 1.98.1/MSVC、Node 24、Python 3.14、FFmpeg 8.0.1 Gyan essentials。安装 Visual Studio C++ Build Tools 和 WebView2 后：

```powershell
npm ci
python -m pip install --target .local/test-deps -r scripts/requirements-dev.txt
python scripts/stage-runtime.py --bin-dir 'C:\path\to\real-ffmpeg-bin'
python scripts/dependency-notices.py
npm run tauri -- dev
```

`--bin-dir` 要求真实的 FFmpeg/ffprobe 文件，不能使用 Chocolatey shim；脚本会检查版本、生成构建信息和 SHA-256。

```powershell
cargo test -p m3u8-core --tests
cargo build -p m3u8tool
python -X utf8 scripts/acceptance.py
npm run build
npm run test:e2e
```

真实桌面测试：先运行 `npm run dev` 和 `cargo build -p m3u8-studio`，另一个终端执行 `node scripts/native-smoke.mjs`。发布构建测试可设置 `M3U8_TEST_EXE` 指向发布 EXE，不需要开发服务器。脚本使用隔离数据目录和自产媒体，通过仅测试时开启的 WebView2 调试端口检查真实 IPC、下载、预览、暂停和取消。生产应用不开放监听端口。

打包、架构和验收记录见 [开发说明](docs/DEVELOPMENT.md)、[验收报告](docs/ACCEPTANCE.md)。本地包尚未签名；公开发布所需材料见 [第三方声明](THIRD_PARTY_NOTICES.md)。

## CLI

```powershell
.\m3u8tool.exe parse --url 'https://example.com/index.m3u8' --format json
.\m3u8tool.exe download --url 'https://example.com/index.m3u8' --output 'D:\Videos\example.mp4'
.\m3u8tool.exe download --resume TASK_ID --ffmpeg 'C:\tools\ffmpeg.exe'
.\m3u8tool.exe download --url URL --request-config request.json --key-file key.bin
.\m3u8tool.exe download --url URL --key-contexts keys.json --keep-cache
```

`request.json` 对应 NetworkConfig；例如 `{"headers":{"Cookie":"..."},"proxy":"socks5h://127.0.0.1:1080","concurrency":8}`。`keys.json` 为 `{"0:1":"十六进制或Base64密钥"}`，上下文编号在解析结果/任务详情中显示。

未标记 AES-128 的来源需显式使用 `--force-key KEY --force-range 100:107`，默认只作用于视频轨道 0。`--allow-gaps` 是 CLI 的明确缺失授权，完成时返回 7；GUI 还会要求查看实际缺失范围后确认。`--overwrite` 明确授权替换同名输出。

优先通过权限受限的配置/密钥文件传入凭证。`--cookie`、`--key` 等明文参数可能进入 shell 历史及进程列表。解析 JSON 会包含用户主动要求查看的原始来源，勿当成脱敏日志分享。

退出码：0 完整成功；2 输入/选择；3 网络/代理/TLS/鉴权；4 解密；5 文件/存储；6 媒体/依赖；7 有缺失；130 中断。下载 stdout 为 JSONL，错误写 stderr。Ctrl+C 保存暂停恢复点。

## 常见问题

- **403/401**：检查授权、链接有效期及源作用域；跨源资源不会自动得到原站 Cookie。
- **来源变化**：不能按相同分片序号混用媒体。保留原任务，核对新来源后复制任务。
- **密钥或媒体无效**：核对上下文、16 字节密钥和 IV；错误填充并不能唯一证明密钥错误。
- **FFmpeg 不可用/输出不兼容**：查看依赖能力和日志；修改外部路径、编码或容器后重新处理缓存。
- **磁盘不足**：释放空间或调整目录后继续；默认低空间阈值为 1 GiB。
- **程序提示实例已打开**：同一数据目录只允许一个引擎写入。请关闭已有实例或使用独立数据目录。

仅处理自己拥有或已获授权的媒体。
