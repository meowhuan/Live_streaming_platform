# 喵喵直播平台

一个完整的直播平台：Rust 后端 + MediaMTX + SRT 推流中继 + WHEP 播放 + 观众登录 + 主题化管理端与观众端。

## 功能特性

- 观众端为首页（`/`），支持 WHEP/HLS 播放与实时弹幕。
- 管理端（`/admin.html`）用于推流配置、链路监控、SMTP 等管理操作。
- MediaMTX 鉴权代理（推流/播放 Token）。
- 内置 SRT 监听/转发，方便本地推流测试。
- 观众账号系统 + 邮箱验证码。
- Turnstile 人机验证。

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

### 观众端接口

- `POST /api/viewer/login`
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

## Turnstile

前端使用：

- Site Key: `TURNSTILE_SITE_SECRET`

后端需要：

- `TURNSTILE_SECRET`

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

### Viewer

- `POST /api/viewer/login`
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

## Turnstile

The frontend uses Cloudflare Turnstile with:

- Site Key: `TURNSTILE_SITE_SECRET`

The backend expects:

- `TURNSTILE_SECRET` in environment

## CF Pages Notes (Frontend)

Cloudflare Pages **does not** read `.env` from the repo. Set Vite variables in:

`Settings → Environment variables`

Use `VITE_*` prefixed vars only.

## License

MIT
