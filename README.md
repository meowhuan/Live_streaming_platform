# 喵喵直播平台

一个完整的直播平台：Rust 后端 + MediaMTX + SRT 推流中继 + WHEP 播放 + 观众登录 + 主题化管理端与观众端。

## 功能特性

- 观众端为首页（`/`），支持 WHEP/HLS 播放与实时弹幕。
- 管理端（`/admin.html`）用于推流配置、链路监控、SMTP 等管理操作。
- MediaMTX 鉴权代理（推流/播放 Token）。
- 内置 SRT 监听/转发，方便本地推流测试。
- 观众账号系统 + 邮箱验证码。
- Turnstile 人机验证。
- 观众邮箱防盗刷（一次性注册令牌 + 频率限制）。
- 可选 Cloudflare Access 管理端二次鉴权。

## 快速开始

1. 创建 MySQL 数据库 `live_streaming`。
2. 复制 `.env.example` → `.env` 并填写。
3. 启动后端：

```bash
cd server-rust
cargo run
```

4. 启动前端：

```bash
npm run dev
```

5. 访问：
- 观众端：`http://localhost:5173/`
- 管理端：`http://localhost:5173/admin.html`

## 后端部署（生产）

以下步骤以 Linux 服务器为例：

1. 安装依赖：
   - Rust（stable）
   - MySQL
   - MediaMTX
2. 创建数据库：
   - 数据库名：`live_streaming`
3. 配置环境变量：
   - 复制 `.env.example` → `.env`
   - 根据实际域名/端口修改（重点：`PORT`、`MEDIAMTX_*`、`TURNSTILE_SECRET`、`CF_ACCESS_*`）
4. 构建后端：

```bash
cd server-rust
cargo build --release
```

5. 启动（示例）：

```bash
./server-rust/target/release/server-rust
```

6. 反向代理（可选，但推荐）：
   - `/api` 与 `/ws` 反代到后端 `PORT`
   - `/live/stream/whep`、`/live/stream` 等播放路径可按需反代至 MediaMTX
   - 需透传 `X-Forwarded-Proto`，后端会根据它生成 HTTPS 播放地址

7. MediaMTX 鉴权：
   - 在 `mediamtx.yml` 中将鉴权回调指向 `POST /api/mediamtx/auth`

8. 端口建议：
   - 后端：`5174`
   - MediaMTX WHEP：`8889`
   - MediaMTX HLS：`8888`
   - MediaMTX SRT：`8890`

如需 systemd 启动服务，可将 `server-rust` 可执行文件放入固定路径，并在 service 中读取 `.env`。

### systemd 服务模板（Linux）

假设后端部署在 `/opt/meow-live`：

```ini
[Unit]
Description=Meow Live Streaming Backend
After=network.target

[Service]
WorkingDirectory=/opt/meow-live/server-rust
ExecStart=/opt/meow-live/server-rust/target/release/server-rust
Restart=on-failure
RestartSec=3
EnvironmentFile=/opt/meow-live/.env

# 安全建议
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true

[Install]
WantedBy=multi-user.target
```

启用方式：

```bash
sudo systemctl daemon-reload
sudo systemctl enable meow-live
sudo systemctl start meow-live
sudo systemctl status meow-live
```

### MediaMTX systemd 服务模板（Linux）

假设 MediaMTX 放在 `/opt/mediamtx`，配置文件为 `/opt/mediamtx/mediamtx.yml`：

```ini
[Unit]
Description=MediaMTX Streaming Server
After=network.target

[Service]
WorkingDirectory=/opt/mediamtx
ExecStart=/opt/mediamtx/mediamtx
Restart=on-failure
RestartSec=3

# 如需指定配置文件：
# ExecStart=/opt/mediamtx/mediamtx /opt/mediamtx/mediamtx.yml

# 安全建议
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true

[Install]
WantedBy=multi-user.target
```

启用方式：

```bash
sudo systemctl daemon-reload
sudo systemctl enable mediamtx
sudo systemctl start mediamtx
sudo systemctl status mediamtx
```

## 环境变量

### 后端（Rust）

- `MYSQL_HOST`, `MYSQL_PORT`, `MYSQL_USER`, `MYSQL_PASSWORD`, `MYSQL_DATABASE`
- `ADMIN_USER`, `ADMIN_PASS`
- `PORT`（默认 `5174`）
- `WS_PATH`（默认 `/ws`）
- `TOKEN_TTL_HOURS`
- `VIEWER_TOKEN_DAYS`
- `TURNSTILE_SECRET`
- `EMAIL_CODE_TTL_MIN`, `EMAIL_ECHO`
- `VIEWER_VERIFY_EMAIL_RATE_LIMIT_WINDOW_SEC`
- `VIEWER_VERIFY_EMAIL_RATE_LIMIT_MAX`
- `VIEWER_VERIFY_EMAIL_RATE_LIMIT_EMAIL_MAX`
- `VIEWER_VERIFY_EMAIL_COOLDOWN_SEC`
- `VIEWER_BLOCK_DISPOSABLE_EMAIL`
- `VIEWER_BLOCK_EDU_GOV_EMAIL`
- `VIEWER_REGISTER_TOKEN_TTL_SEC`
- `CF_ACCESS_TEAM_DOMAIN`, `CF_ACCESS_AUD`
- `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`, `SMTP_PASS`, `SMTP_FROM`, `SMTP_REPLY_TO`, `SMTP_STARTTLS`
- `MEDIAMTX_API`, `MEDIAMTX_METRICS`, `MEDIAMTX_WEBRTC`, `MEDIAMTX_HLS`, `MEDIAMTX_PATH`, `MEDIAMTX_POLL_MS`
- `SRT_LISTEN`, `SRT_FORWARD`

### 前端（Vite）

- `VITE_WS_URL`（可覆盖 WS 地址）

## API 接口

### 公共读接口

- `GET /api/stream`
- `GET /api/stream/stats`
- `GET /api/stream/health`
- `GET /api/ingest/config`
- `GET /api/stream/ingest`
- `GET /api/stream/play`
- `GET /api/schedule`
- `GET /api/channels`
- `GET /api/ops/alerts`
- `GET /api/ingest/nodes`
- `GET /api/chat/latest`
- `GET /api/mediamtx/metrics`

### 管理端接口（需登录）

- `POST /api/admin/login`
- `POST /api/admin/stream/update`
- `POST /api/admin/stats/replace`
- `POST /api/admin/health/replace`
- `POST /api/admin/schedule/replace`
- `POST /api/admin/channels/replace`
- `POST /api/admin/ops/replace`
- `POST /api/admin/nodes/replace`
- `POST /api/admin/ingest/replace`
- `POST /api/admin/room/create`
- `POST /api/admin/smtp`（保存 SMTP）
- `GET /api/admin/smtp`（读取 SMTP）
- `POST /api/admin/smtp/test`（测试邮件）
- `GET /api/admin/viewer-anti-abuse`（读取防盗刷配置）
- `POST /api/admin/viewer-anti-abuse`（保存防盗刷配置）

### 观众端接口

- `POST /api/viewer/login`
- `POST /api/viewer/register/token`
- `POST /api/viewer/register/start`
- `POST /api/viewer/register/verify`
- `GET /api/viewer/me`
- `GET /api/viewer/profile`
- `POST /api/viewer/profile`

### 弹幕接口

- `POST /api/chat/send`（需要观众登录）

## WebSocket

- `ws://localhost:5174/ws`
- 连接后发送 `{ type: "snapshot", data: { ... } }`
- 更新推送：`stream:update`, `chat:new`, `ops:update` 等

## MediaMTX 鉴权代理

MediaMTX 指向：

- `POST /api/mediamtx/auth`

后端会校验 publish/read Token。

## SRT 中继

后端监听 `SRT_LISTEN`（默认 `0.0.0.0:9000`）
可选转发：

```
SRT_FORWARD=srt://127.0.0.1:9001?mode=caller
```

## 推流 Token 策略

- 推流 Token 会在首次启动时生成并持久化保存（重启不会改变）。
- 如需手动刷新推流 Key，可在管理端点击“刷新推流 Key”，或调用：
  - `POST /api/admin/ingest/refresh`

## Turnstile

前端使用：

- Site Key: `TURNSTILE_SITE_KEY`

后端需要：

- `TURNSTILE_SECRET`

## Cloudflare Access（可选）

当配置 `CF_ACCESS_TEAM_DOMAIN` 和 `CF_ACCESS_AUD` 时，管理端接口需要同时满足：

- Admin Bearer Token
- `CF-Access-Jwt-Assertion` 头（由 Cloudflare Access 注入）

## CF Pages 部署（前端）

Cloudflare Pages **不会读取** 仓库里的 `.env`。请在：

`Settings → Environment variables`

配置 `VITE_*` 前缀变量。

## 许可

MIT

---

# Meowhuan Live Streaming Platform

A full-stack live streaming platform with a Rust backend, MediaMTX integration, SRT ingest relay, WHEP playback, viewer login, and a themed admin + viewer UI.

## Features

- Viewer page as homepage (`/`) with WHEP/HLS playback and realtime chat.
- Admin console (`/admin.html`) for stream config, ingest data, metrics, and SMTP settings.
- MediaMTX auth proxy for publish/read tokens.
- Built-in SRT listener/relay for ingest testing.
- Viewer accounts with email verification.
- Turnstile verification for admin + viewer auth.
- Viewer email anti-abuse (one-time register token + rate limits).
- Optional Cloudflare Access for admin APIs.

## Quick Start

1. Create MySQL database `live_streaming`.
2. Copy `.env.example` → `.env` and update values.
3. Start backend:

```bash
cd server-rust
cargo run
```

4. Start frontend:

```bash
npm run dev
```

5. Visit:
- Viewer: `http://localhost:5173/`
- Admin: `http://localhost:5173/admin.html`

## Backend Deployment (Production)

Example for Linux servers:

1. Install dependencies:
   - Rust (stable)
   - MySQL
   - MediaMTX
2. Create database:
   - name: `live_streaming`
3. Configure environment:
   - copy `.env.example` → `.env`
   - update `PORT`, `MEDIAMTX_*`, `TURNSTILE_SECRET`, `CF_ACCESS_*`
4. Build backend:

```bash
cd server-rust
cargo build --release
```

5. Run:

```bash
./server-rust/target/release/server-rust
```

6. Reverse proxy (recommended):
   - proxy `/api` and `/ws` to backend `PORT`
   - proxy `/live/stream/whep` or other playback paths to MediaMTX if needed
   - pass `X-Forwarded-Proto` so the backend can emit HTTPS playback URLs

7. MediaMTX auth:
   - set auth hook to `POST /api/mediamtx/auth`

8. Suggested ports:
   - backend: `5174`
   - MediaMTX WHEP: `8889`
   - MediaMTX HLS: `8888`
   - MediaMTX SRT: `8890`

If you use systemd, place the binary in a fixed path and load `.env` in the service unit.

### systemd Service Template (Linux)

Assuming the backend lives in `/opt/meow-live`:

```ini
[Unit]
Description=Meow Live Streaming Backend
After=network.target

[Service]
WorkingDirectory=/opt/meow-live/server-rust
ExecStart=/opt/meow-live/server-rust/target/release/server-rust
Restart=on-failure
RestartSec=3
EnvironmentFile=/opt/meow-live/.env

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable meow-live
sudo systemctl start meow-live
sudo systemctl status meow-live
```

### MediaMTX systemd Service Template (Linux)

Assuming MediaMTX is installed at `/opt/mediamtx` and config is `/opt/mediamtx/mediamtx.yml`:

```ini
[Unit]
Description=MediaMTX Streaming Server
After=network.target

[Service]
WorkingDirectory=/opt/mediamtx
ExecStart=/opt/mediamtx/mediamtx
Restart=on-failure
RestartSec=3

# If you want to specify the config file:
# ExecStart=/opt/mediamtx/mediamtx /opt/mediamtx/mediamtx.yml

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable mediamtx
sudo systemctl start mediamtx
sudo systemctl status mediamtx
```

## Environment Variables

### Backend (Rust)

- `MYSQL_HOST`, `MYSQL_PORT`, `MYSQL_USER`, `MYSQL_PASSWORD`, `MYSQL_DATABASE`
- `ADMIN_USER`, `ADMIN_PASS`
- `PORT` (default `5174`)
- `WS_PATH` (default `/ws`)
- `TOKEN_TTL_HOURS`
- `VIEWER_TOKEN_DAYS`
- `TURNSTILE_SECRET`
- `EMAIL_CODE_TTL_MIN`, `EMAIL_ECHO`
- `VIEWER_VERIFY_EMAIL_RATE_LIMIT_WINDOW_SEC`
- `VIEWER_VERIFY_EMAIL_RATE_LIMIT_MAX`
- `VIEWER_VERIFY_EMAIL_RATE_LIMIT_EMAIL_MAX`
- `VIEWER_VERIFY_EMAIL_COOLDOWN_SEC`
- `VIEWER_BLOCK_DISPOSABLE_EMAIL`
- `VIEWER_BLOCK_EDU_GOV_EMAIL`
- `VIEWER_REGISTER_TOKEN_TTL_SEC`
- `CF_ACCESS_TEAM_DOMAIN`, `CF_ACCESS_AUD`
- `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`, `SMTP_PASS`, `SMTP_FROM`, `SMTP_REPLY_TO`, `SMTP_STARTTLS`
- `MEDIAMTX_API`, `MEDIAMTX_METRICS`, `MEDIAMTX_WEBRTC`, `MEDIAMTX_HLS`, `MEDIAMTX_PATH`, `MEDIAMTX_POLL_MS`
- `SRT_LISTEN`, `SRT_FORWARD`

### Frontend (Vite)

- `VITE_WS_URL` (override WS endpoint)

## Endpoints

### Public (read)

- `GET /api/stream`
- `GET /api/stream/stats`
- `GET /api/stream/health`
- `GET /api/ingest/config`
- `GET /api/stream/ingest`
- `GET /api/stream/play`
- `GET /api/schedule`
- `GET /api/channels`
- `GET /api/ops/alerts`
- `GET /api/ingest/nodes`
- `GET /api/chat/latest`
- `GET /api/mediamtx/metrics`

### Admin (auth required)

- `POST /api/admin/login`
- `POST /api/admin/stream/update`
- `POST /api/admin/stats/replace`
- `POST /api/admin/health/replace`
- `POST /api/admin/schedule/replace`
- `POST /api/admin/channels/replace`
- `POST /api/admin/ops/replace`
- `POST /api/admin/nodes/replace`
- `POST /api/admin/ingest/replace`
- `POST /api/admin/room/create`
- `POST /api/admin/smtp` (save SMTP)
- `GET /api/admin/smtp` (load SMTP)
- `POST /api/admin/smtp/test`
- `GET /api/admin/viewer-anti-abuse`
- `POST /api/admin/viewer-anti-abuse`

### Viewer

- `POST /api/viewer/login`
- `POST /api/viewer/register/token`
- `POST /api/viewer/register/start`
- `POST /api/viewer/register/verify`
- `GET /api/viewer/me`
- `GET /api/viewer/profile`
- `POST /api/viewer/profile`

### Chat

- `POST /api/chat/send` (viewer auth required)

## WebSocket

- `ws://localhost:5174/ws`
- Sends snapshot on connect: `{ type: "snapshot", data: { ... } }`
- Broadcasts updates: `stream:update`, `chat:new`, `ops:update`, etc.

## MediaMTX Auth Proxy

Configure MediaMTX to call:

- `POST /api/mediamtx/auth`

The backend validates `token` for publish/read actions.

## SRT Relay

The Rust backend listens on `SRT_LISTEN` (default `0.0.0.0:9000`).
Optional forward target:

```
SRT_FORWARD=srt://127.0.0.1:9001?mode=caller
```

## Ingest Token Strategy

- The publish token is generated once and persisted (it won’t change after restarts).
- To rotate it manually, click “刷新推流 Key” in the admin UI or call:
  - `POST /api/admin/ingest/refresh`

## Turnstile

The frontend uses Cloudflare Turnstile with:

- Site Key: `TURNSTILE_SITE_KEY`

The backend expects:

- `TURNSTILE_SECRET` in environment

## Cloudflare Access (Optional)

When `CF_ACCESS_TEAM_DOMAIN` and `CF_ACCESS_AUD` are set, admin APIs require both:

- Admin Bearer Token
- `CF-Access-Jwt-Assertion` header (injected by Cloudflare Access)

## CF Pages Notes (Frontend)

Cloudflare Pages **does not** read `.env` from the repo. Set Vite variables in:

`Settings → Environment variables`

Use `VITE_*` prefixed vars only.

## License

MIT
