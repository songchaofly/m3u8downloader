# 第三方组件与本地构建说明

本项目应用代码按 GPL-3.0-or-later 提供，全文见 LICENSE。第三方组件保持各自许可证。锁定依赖见 Cargo.lock、package-lock.json；构建随附依赖清单由 scripts/dependency-notices.py 从实际依赖生成。

## 媒体工具

本地验收包使用 FFmpeg / ffprobe `8.0.1-essentials_build-www.gyan.dev`，Gyan Windows x64 静态构建。构建启用了 `--enable-gpl --enable-version3`，包括 libx264、libx265 和 AAC 能力。实际文件 SHA-256、字节数及完整 `-version` 输出保存在 `runtime/FFMPEG-BUILD.json`，由 scripts/stage-runtime.py 生成。

- 上游：https://ffmpeg.org/ ；固定版本源码：https://github.com/FFmpeg/FFmpeg/tree/n8.0.1
- 构建提供方：https://www.gyan.dev/ffmpeg/builds/
- FFmpeg 许可说明：licenses/FFmpeg-LICENSE.md
- GPL v3 全文：licenses/GPL-3.0.txt

本目录生成的安装包和便携包用于本地开发验收，未签名。以上上游链接不是完整对应源码的替代品，也不代表已经审查了此静态构建全部外部库的分发材料。对外发布前，发布者需准备与此二进制匹配的 FFmpeg、静态依赖库、修改和构建脚本等完整对应源码，并核验全部许可、版权和专利授权条件。此项是公开发布门槛，不应将本地构建成功误记为发行审核通过。

## 桌面与前端

Tauri（MIT / Apache-2.0）、Vue（MIT）、hls.js（Apache-2.0）、Lucide（ISC）及其他直接、间接依赖以实际包内许可证为准。自动生成清单包括依赖名称、锁定版本、声明许可证和包内许可证副本；不能仅凭 SPDX 声明推断已经完成法律审查。

Windows 桌面使用 Microsoft Edge WebView2。NSIS 安装包配置为携带微软的离线安装器；WebView2 按微软单独的许可条款提供。应用未包含任何第三方网站媒体、用户凭证或遥测服务。
