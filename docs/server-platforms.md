# 服务端跨平台运行说明

Rust 服务端支持 Linux、Windows 和 macOS。发行包只包含后端二进制、示例环境变量、文档和运行目录；前端仍按 Vite/Cloudflare Pages 等方式单独构建部署。

## 运行依赖

- MySQL 8.x 或兼容版本。
- MediaMTX，用于 WHEP/HLS/RTMP/RTSP/SRT 媒体服务。
- FFmpeg，仅在使用直播封面截图或剪辑功能时需要。
- Rust stable，仅源码构建时需要；使用 Release 发行包不需要安装 Rust。

## 配置文件

复制 `.env.example` 为 `.env`，并按平台调整以下变量：

- `PORT`: HTTP/WebSocket 服务端口，默认 `5174`。
- `BIND_HOST`: 监听地址，默认 `0.0.0.0`。仅本机访问可设为 `127.0.0.1`，IPv6 可设为 `[::]`。
- `SRT_ENABLED`: 是否启用内置 SRT 监听，默认 `true`。如果只使用 MediaMTX 推流，可设为 `false`。
- `SRT_LISTEN`: 内置 SRT 监听地址，默认 `0.0.0.0:9000`。
- `CLIP_DIR`: 剪辑输出目录，默认 `public/clips`。
- `CLIP_TMP_DIR`: 剪辑临时目录，默认使用系统临时目录下的 `meow-live-clip-tmp`。
- `LIVE_DIR`: 直播封面等静态输出目录，默认 `public/live`。
- `FFMPEG_BIN`: FFmpeg 可执行文件名或绝对路径，Windows 可使用 `C:\ffmpeg\bin\ffmpeg.exe`。
- `THUMBNAIL_ENABLED`: 是否启用直播封面截图循环，默认 `true`。没有安装 FFmpeg 时建议设为 `false`。

相对路径均相对于服务端进程的当前工作目录。作为系统服务运行时，建议把 `CLIP_DIR`、`CLIP_TMP_DIR`、`LIVE_DIR`、`FFMPEG_BIN` 配成绝对路径，避免服务管理器的工作目录差异。

## Windows

源码构建：

```powershell
cd server-rust
cargo build --release
.\target\release\server-rust.exe
```

发行包运行：

```powershell
Copy-Item .env.example .env
.\server-rust.exe
```

常用 PowerShell 环境变量示例：

```powershell
$env:PORT = "5174"
$env:BIND_HOST = "127.0.0.1"
$env:SRT_ENABLED = "false"
$env:THUMBNAIL_ENABLED = "false"
.\server-rust.exe
```

首次监听公网地址时，Windows 防火墙可能弹出网络访问确认。仅本机反向代理访问时，将 `BIND_HOST=127.0.0.1` 可以减少暴露面。

## macOS

源码构建：

```bash
cd server-rust
cargo build --release
./target/release/server-rust
```

发行包运行：

```bash
cp .env.example .env
chmod +x ./server-rust
./server-rust
```

从浏览器下载的未签名二进制可能带有 quarantine 标记。如确认来源可信，可在发行包目录执行：

```bash
xattr -dr com.apple.quarantine ./server-rust
```

macOS 发行包使用 universal binary，同时包含 Intel 和 Apple Silicon 架构。

## Linux

源码构建：

```bash
cd server-rust
cargo build --release
./target/release/server-rust
```

生产环境仍推荐使用 systemd 托管，并把 `.env` 放在固定路径。systemd 示例见仓库根目录 `README.md`。

## CI 与发行

- `CI` 工作流会构建前端，并在 Linux、Windows、macOS 上对 Rust 服务端运行 `fmt`、`clippy` 和 `test`。
- `Release Server` 工作流在推送 `v*` tag 时触发，也支持手动输入 tag 触发。
- 发行产物：
  - `meow-live-server-linux-x64.tar.gz`
  - `meow-live-server-windows-x64.zip`
  - `meow-live-server-macos-universal.tar.gz`

手动发版示例：

```bash
git tag v0.1.0
git push origin v0.1.0
```

---

# Server Platform Notes

The Rust backend supports Linux, Windows, and macOS. Release packages include the backend binary, sample environment file, docs, and runtime directories. The frontend is still built and deployed separately.

## Runtime Requirements

- MySQL 8.x or a compatible server.
- MediaMTX for WHEP/HLS/RTMP/RTSP/SRT media services.
- FFmpeg only when thumbnails or clip creation are enabled.
- Rust stable only when building from source.

## Key Platform Settings

- `PORT`: HTTP/WebSocket port, default `5174`.
- `BIND_HOST`: bind address, default `0.0.0.0`; use `127.0.0.1` for local reverse proxy only, or `[::]` for IPv6.
- `SRT_ENABLED`: enable the built-in SRT listener, default `true`.
- `SRT_LISTEN`: built-in SRT listen address, default `0.0.0.0:9000`.
- `CLIP_DIR`: clip output directory, default `public/clips`.
- `CLIP_TMP_DIR`: clip temp directory, default system temp directory plus `meow-live-clip-tmp`.
- `LIVE_DIR`: live static output directory, default `public/live`.
- `FFMPEG_BIN`: FFmpeg executable name or absolute path.
- `THUMBNAIL_ENABLED`: enable the thumbnail capture loop, default `true`.

Relative paths are resolved from the server process working directory. For service-managed deployments, prefer absolute paths.
