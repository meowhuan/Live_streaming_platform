use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use dotenvy::dotenv;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use futures::{SinkExt, StreamExt};
use srt_tokio::SrtSocket;
use sqlx::{mysql::MySqlPoolOptions, MySqlPool};
use reqwest::Client;
use sha2::{Digest, Sha256};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message as MailMessage,
    transport::smtp::authentication::Credentials,
    Tokio1Executor,
};
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::info;
use tracing_subscriber::fmt::Subscriber;

struct AppState {
    pool: MySqlPool,
    admin_user: String,
    admin_pass: String,
    token_ttl: Duration,
    broadcaster: broadcast::Sender<String>,
    active_streams: AtomicUsize,
    tokens: RwLock<HashMap<String, Instant>>,
    viewer_tokens: RwLock<HashMap<String, ViewerSession>>,
    viewer_token_ttl: Duration,
    publish_token: RwLock<String>,
    read_token: RwLock<String>,
    mediamtx_api: String,
    mediamtx_metrics: String,
    mediamtx_path: String,
    mediamtx_webrtc: String,
    mediamtx_hls: String,
    pending_viewer_codes: RwLock<HashMap<String, PendingCode>>,
    email_code_ttl: Duration,
    email_echo: bool,
    smtp: RwLock<Option<SmtpConfig>>,
    metrics_state: RwLock<MetricsState>,
    turnstile_secret: Option<String>,
}

#[derive(Deserialize)]
struct StreamUpdate {
    #[serde(rename = "roomId")]
    room_id: Option<String>,
    title: Option<String>,
    host: Option<String>,
    resolution: Option<String>,
    status: Option<String>,
    bitrate: Option<String>,
    latency: Option<String>,
    #[serde(rename = "chatRate")]
    chat_rate: Option<String>,
    #[serde(rename = "giftResponse")]
    gift_response: Option<String>,
    #[serde(rename = "giftLatency")]
    gift_latency: Option<String>,
    viewers: Option<f64>,
}

#[derive(Deserialize)]
struct LoginPayload {
    username: String,
    password: String,
    #[serde(rename = "turnstileToken")]
    turnstile_token: Option<String>,
}

#[derive(Deserialize)]
struct RegisterStartPayload {
    email: String,
    #[serde(rename = "turnstileToken")]
    turnstile_token: Option<String>,
}

#[derive(Deserialize)]
struct RegisterVerifyPayload {
    email: String,
    code: String,
    username: String,
    password: String,
    #[serde(rename = "turnstileToken")]
    turnstile_token: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct AdminAccount {
    email: String,
    username: String,
    password_hash: String,
    created_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct ViewerAccount {
    email: String,
    username: String,
    password_hash: String,
    created_at: String,
}

struct PendingCode {
    code: String,
    expires_at: Instant,
}

struct ViewerSession {
    username: String,
    expires_at: Instant,
}

#[derive(Serialize, Deserialize, Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
    from: String,
    reply_to: Option<String>,
    starttls: bool,
}

#[derive(Serialize)]
struct SmtpPublic {
    host: String,
    port: u16,
    username: String,
    from: String,
    reply_to: Option<String>,
    starttls: bool,
    password_set: bool,
}

struct MetricsState {
    last_rx_bytes: Option<f64>,
    last_tx_bytes: Option<f64>,
    last_ts: Instant,
}

#[derive(Deserialize)]
struct AuthRequest {
    user: Option<String>,
    password: Option<String>,
    action: String,
    path: Option<String>,
    token: Option<String>,
    query: Option<String>,
}

#[derive(Deserialize)]
struct ChatSend {
    text: String,
}

#[derive(Deserialize)]
struct SmtpTestPayload {
    to: String,
}

#[derive(Serialize)]
struct ViewerProfile {
    username: String,
    email: String,
}

#[derive(Deserialize)]
struct ViewerProfileUpdate {
    username: String,
    email: String,
}

#[derive(Deserialize)]
struct TelemetryPayload {
    #[serde(rename = "e2eLatencyMs")]
    e2e_latency_ms: Option<f64>,
    #[serde(rename = "playoutDelayMs")]
    playout_delay_ms: Option<f64>,
    #[serde(rename = "pushLatencyMs")]
    push_latency_ms: Option<f64>,
}

#[derive(Serialize, sqlx::FromRow)]
struct ChatMessage {
    id: i64,
    user: String,
    text: String,
    created_at: String,
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    Subscriber::builder().with_target(false).init();

    let port = env_u16("PORT", 5174);
    let admin_user = std::env::var("ADMIN_USER").unwrap_or_else(|_| "admin".to_string());
    let admin_pass = std::env::var("ADMIN_PASS").unwrap_or_else(|_| "admin123".to_string());
    let token_ttl = Duration::from_secs(env_u64("TOKEN_TTL_HOURS", 12) * 3600);
    let ws_path = std::env::var("WS_PATH").unwrap_or_else(|_| "/ws".to_string());
    let srt_listen = std::env::var("SRT_LISTEN").unwrap_or_else(|_| "0.0.0.0:9000".to_string());
    let srt_forward = std::env::var("SRT_FORWARD").ok().filter(|val| !val.trim().is_empty());
    let mediamtx_api = std::env::var("MEDIAMTX_API").unwrap_or_else(|_| "http://127.0.0.1:9997".to_string());
    let mediamtx_metrics = std::env::var("MEDIAMTX_METRICS").unwrap_or_else(|_| "http://127.0.0.1:9998".to_string());
    let mediamtx_path = std::env::var("MEDIAMTX_PATH").unwrap_or_else(|_| "live/stream".to_string());
    let mediamtx_webrtc = std::env::var("MEDIAMTX_WEBRTC").unwrap_or_else(|_| "http://127.0.0.1:8889".to_string());
    let mediamtx_hls = std::env::var("MEDIAMTX_HLS").unwrap_or_else(|_| "http://127.0.0.1:8888".to_string());
    let mediamtx_poll = env_u64("MEDIAMTX_POLL_MS", 2000);
    let email_code_ttl = Duration::from_secs(env_u64("EMAIL_CODE_TTL_MIN", 10) * 60);
    let email_echo = std::env::var("EMAIL_ECHO")
        .map(|val| val.to_lowercase() != "false")
        .unwrap_or(true);
    let turnstile_secret = std::env::var("TURNSTILE_SECRET").ok().filter(|val| !val.trim().is_empty());
    let viewer_token_ttl = Duration::from_secs(env_u64("VIEWER_TOKEN_DAYS", 30) * 24 * 3600);

    let pool = create_pool().await;
    ensure_tables(&pool).await;
    ensure_defaults(&pool).await;
    let smtp = load_smtp_config_db(&pool).await.or_else(load_smtp_config);

    let (tx, _rx) = broadcast::channel::<String>(64);
    let state = AppState {
        pool,
        admin_user,
        admin_pass,
        token_ttl,
        broadcaster: tx,
        active_streams: AtomicUsize::new(0),
        tokens: RwLock::new(HashMap::new()),
        viewer_tokens: RwLock::new(HashMap::new()),
        viewer_token_ttl,
        publish_token: RwLock::new(String::new()),
        read_token: RwLock::new(String::new()),
        mediamtx_api,
        mediamtx_metrics,
        mediamtx_path,
        mediamtx_webrtc,
        mediamtx_hls,
        pending_viewer_codes: RwLock::new(HashMap::new()),
        email_code_ttl,
        email_echo,
        smtp: RwLock::new(smtp),
        metrics_state: RwLock::new(MetricsState {
            last_rx_bytes: None,
            last_tx_bytes: None,
            last_ts: Instant::now(),
        }),
        turnstile_secret,
    };

    {
        let mut publish = state.publish_token.write().await;
        if publish.is_empty() {
            *publish = generate_key();
        }
        let mut read = state.read_token.write().await;
        if read.is_empty() {
            *read = generate_key();
        }
    }

    let router = Router::new()
        .route("/api/stream", get(get_stream))
        .route("/api/stream/stats", get(get_stats))
        .route("/api/stream/health", get(get_health))
        .route("/api/ingest/config", get(get_ingest))
        .route("/api/stream/ingest", get(get_stream_ingest))
        .route("/api/stream/play", get(get_stream_play))
        .route("/api/stream/telemetry", post(post_stream_telemetry))
        .route("/api/schedule", get(get_schedule))
        .route("/api/channels", get(get_channels))
        .route("/api/ops/alerts", get(get_ops))
        .route("/api/ingest/nodes", get(get_nodes))
        .route("/api/chat/latest", get(get_chat_latest))
        .route("/api/chat/send", post(post_chat_send))
        .route("/api/mediamtx/auth", post(mediamtx_auth))
        .route("/api/mediamtx/metrics", get(get_mediamtx_metrics))
        .route("/api/admin/login", post(admin_login))
        .route("/api/admin/smtp", get(admin_smtp_get))
        .route("/api/admin/smtp", post(admin_smtp_set))
        .route("/api/admin/smtp/test", post(admin_smtp_test))
        .route("/api/viewer/login", post(viewer_login))
        .route("/api/viewer/register/start", post(viewer_register_start))
        .route("/api/viewer/register/verify", post(viewer_register_verify))
        .route("/api/viewer/me", get(viewer_me))
        .route("/api/viewer/profile", get(viewer_profile_get))
        .route("/api/viewer/profile", post(viewer_profile_update))
        .route("/api/admin/stream/update", post(admin_stream_update))
        .route("/api/admin/stats/replace", post(admin_stats_replace))
        .route("/api/admin/health/replace", post(admin_health_replace))
        .route("/api/admin/schedule/replace", post(admin_schedule_replace))
        .route("/api/admin/channels/replace", post(admin_channels_replace))
        .route("/api/admin/ops/replace", post(admin_ops_replace))
        .route("/api/admin/nodes/replace", post(admin_nodes_replace))
        .route("/api/admin/ingest/replace", post(admin_ingest_replace))
        .route("/api/admin/room/create", post(admin_room_create))
        .route("/api/ingest/report", post(report_ingest))
        .route(&ws_path, get(ws_handler))
        .layer(CorsLayer::permissive());

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Rust backend running at http://localhost:{port}");
    info!("WebSocket server at ws://localhost:{port}{ws_path}");
    let state_for_srt = Arc::new(state);
    let srt_state = state_for_srt.clone();
    tokio::spawn(async move {
        if let Err(err) = run_srt_listener(&srt_state, &srt_listen, srt_forward.as_deref()).await {
            info!("SRT listener stopped: {err}");
        }
    });

    let api_state = state_for_srt.clone();
    tokio::spawn(async move {
        run_mediamtx_poll(api_state, mediamtx_poll).await;
    });

    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), router.with_state(state_for_srt))
        .await
        .unwrap();
}

async fn create_pool() -> MySqlPool {
    let host = std::env::var("MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env_u16("MYSQL_PORT", 3306);
    let user = std::env::var("MYSQL_USER").unwrap_or_else(|_| "root".to_string());
    let password = std::env::var("MYSQL_PASSWORD").unwrap_or_default();
    let database = std::env::var("MYSQL_DATABASE").unwrap_or_else(|_| "live_streaming".to_string());
    let url = format!("mysql://{user}:{password}@{host}:{port}/{database}");

    MySqlPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("MySQL connect failed")
}

async fn ensure_tables(pool: &MySqlPool) {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS kv_store (
          k VARCHAR(64) PRIMARY KEY,
          v LONGTEXT NOT NULL,
          updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
        "#,
    )
    .execute(pool)
    .await
    .expect("create table failed");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS chat_messages (
          id BIGINT PRIMARY KEY AUTO_INCREMENT,
          user VARCHAR(64) NOT NULL,
          text VARCHAR(280) NOT NULL,
          created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
        "#,
    )
    .execute(pool)
    .await
    .expect("create chat table failed");
}

async fn ensure_defaults(pool: &MySqlPool) {
    for (key, value) in default_kv() {
        let existing: Option<String> = sqlx::query_scalar("SELECT v FROM kv_store WHERE k = ? LIMIT 1")
            .bind(&key)
            .fetch_optional(pool)
            .await
            .expect("select default failed");
        if existing.is_none() {
            set_json(pool, &key, &value).await;
        }
    }
}

async fn get_json(pool: &MySqlPool, key: &str, fallback: Value) -> Value {
    let row: Option<String> = sqlx::query_scalar("SELECT v FROM kv_store WHERE k = ? LIMIT 1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);
    if let Some(raw) = row {
        serde_json::from_str(&raw).unwrap_or(fallback)
    } else {
        fallback
    }
}

async fn set_json(pool: &MySqlPool, key: &str, value: &Value) {
    let payload = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    sqlx::query(
        "INSERT INTO kv_store (k, v) VALUES (?, ?) ON DUPLICATE KEY UPDATE v = VALUES(v)",
    )
    .bind(key)
    .bind(payload)
    .execute(pool)
    .await
    .expect("insert failed");
}

fn default_kv() -> Vec<(String, Value)> {
    let stream = json!({
        "roomId": "1024",
        "title": "夜间陪伴频道",
        "host": "Meowhuan",
        "resolution": "1080p",
        "status": "live",
        "bitrate": "8.4 Mbps",
        "latency": "2.3s",
        "pushLatency": "0.8s",
        "playoutDelay": "420ms",
        "chatRate": "132 / min",
        "giftResponse": "98.2%",
        "giftLatency": "0.4s",
        "viewers": 12842,
        "updatedAt": now_iso()
    });

    let stats = json!([
        { "label": "在线观众", "value": "12,842", "note": "+8%", "tone": "good" },
        { "label": "推流带宽", "value": "8.4 Mbps", "note": "稳定", "tone": "good" },
        { "label": "端到端延迟", "value": "2.3s", "note": "目标 < 3s", "tone": "good" },
        { "label": "丢帧率", "value": "0.28%", "note": "可接受", "tone": "warn" }
    ]);

    let health = json!([
        { "label": "推流包完整度", "value": 92, "note": "SRT 正常" },
        { "label": "编码器输出", "value": 88, "note": "OBS / 1080p" },
        { "label": "节点回传", "value": 78, "note": "华南边缘" },
        { "label": "CDN 命中率", "value": 95, "note": "99.1% 缓存" }
    ]);

    let ingest = json!([
        {
            "protocol": "SRT",
            "url": "srt://127.0.0.1:9000",
            "note": "本地测试线路（监听端口 9000）",
            "tag": "本地测试"
        },
        {
            "protocol": "OBS",
            "url": "obs://profile/live",
            "note": "编码器：H.264 / 1080p60",
            "tag": "推流配置"
        },
        {
            "label": "Key",
            "value": "meow_live_2026_****",
            "note": "有效期：24 小时",
            "tag": "已隐藏"
        }
    ]);

    let schedule = json!([
        { "time": "20:00", "title": "《夜猫直播间》例行开播", "tag": "常规", "host": "Meowhuan" },
        { "time": "21:30", "title": "联动访谈：新主播见面会", "tag": "联动", "host": "Meowhuan & Alice" },
        { "time": "23:00", "title": "深夜安静电台", "tag": "放松", "host": "Meowhuan" }
    ]);

    let channels = json!([
        { "name": "主直播间", "category": "聊天 / 日常", "followers": "84.6k", "status": "直播中" },
        { "name": "游戏观察室", "category": "游戏 / 策略", "followers": "26.9k", "status": "准备中" },
        { "name": "学习陪伴区", "category": "学习 / 纯音乐", "followers": "13.2k", "status": "休息" }
    ]);

    let ops = json!([
        { "title": "麦克风噪声偏高", "desc": "建议检查输入增益，噪声门 12dB。" },
        { "title": "备用推流未启动", "desc": "建议开启备用推流保护线路。" },
        { "title": "弹幕节奏上升", "desc": "建议增加慢速模式 +10 秒。" }
    ]);

    let nodes = json!([
        { "city": "广州", "latency": "12ms", "load": "低" },
        { "city": "深圳", "latency": "18ms", "load": "中" },
        { "city": "上海", "latency": "33ms", "load": "中" },
        { "city": "成都", "latency": "46ms", "load": "高" }
    ]);

    vec![
        ("stream".to_string(), stream),
        ("stats".to_string(), stats),
        ("health".to_string(), health),
        ("ingest_config".to_string(), ingest),
        ("schedule".to_string(), schedule),
        ("channels".to_string(), channels),
        ("ops_alerts".to_string(), ops),
        ("nodes".to_string(), nodes),
    ]
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn env_u16(key: &str, fallback: u16) -> u16 {
    std::env::var(key)
        .ok()
        .and_then(|val| val.parse::<u16>().ok())
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|val| val.parse::<u64>().ok())
        .unwrap_or(fallback)
}

async fn run_mediamtx_poll(
    state: Arc<AppState>,
    interval_ms: u64,
) {
    let client = Client::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(interval_ms.max(500)));
    loop {
        ticker.tick().await;
        let url = format!("{}/v3/paths/list", state.mediamtx_api.trim_end_matches('/'));
        let resp = client.get(&url).send().await;
        let Ok(resp) = resp else { continue };
        let json = resp.json::<Value>().await;
        let Ok(json) = json else { continue };
        let items = json
            .get("items")
            .or_else(|| json.get("paths"))
            .and_then(|v| v.as_array());
        let Some(items) = items else { continue };

        let mut status = "offline".to_string();
        let mut viewers: Option<f64> = None;
        let mut bitrate: Option<String> = None;
        let mut push_latency: Option<String> = None;
        for item in items {
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name != state.mediamtx_path {
                continue;
            }
            let ready = item
                .get("ready")
                .and_then(|v| v.as_bool())
                .or_else(|| item.get("sourceReady").and_then(|v| v.as_bool()))
                .unwrap_or(false);
            status = if ready { "live" } else { "offline" }.to_string();
            if let Some(readers) = item.get("readers").and_then(|v| v.as_array()) {
                viewers = Some(readers.len() as f64);
            }
            break;
        }

        if let Some(metrics) = fetch_mediamtx_metrics(&client, &state).await {
            let rx = metric_value_for_path(
                &metrics,
                &[
                    "mediamtx_path_bytes_received_total",
                    "mediamtx_path_source_bytes_received_total",
                ],
                &state.mediamtx_path,
            );
            let tx = metric_value_for_path(
                &metrics,
                &["mediamtx_path_bytes_sent_total"],
                &state.mediamtx_path,
            );
            if let Some(latency) = metric_value_for_path(
                &metrics,
                &["mediamtx_path_source_latency_seconds", "mediamtx_path_source_latency"],
                &state.mediamtx_path,
            ) {
                push_latency = Some(format!("{:.2}s", latency));
            }
            if let Some(mbps) = compute_bitrate_mbps(&state, rx, tx).await {
                bitrate = Some(format!("{:.2} Mbps", mbps));
            }
        }

        update_stream_fields(&state, status, viewers, bitrate, push_latency).await;
    }
}

async fn get_stream(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    if let Some(obj) = stream.as_object_mut() {
        obj.insert("updatedAt".to_string(), json!(now_iso()));
    }
    set_json(&state.pool, "stream", &stream).await;
    Json(stream)
}

async fn get_stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stats = get_json(&state.pool, "stats", default_kv()[1].1.clone()).await;
    Json(json!({ "updatedAt": now_iso(), "items": stats }))
}

async fn get_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let health = get_json(&state.pool, "health", default_kv()[2].1.clone()).await;
    Json(json!({ "updatedAt": now_iso(), "items": health }))
}

async fn get_ingest(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let ingest = get_json(&state.pool, "ingest_config", default_kv()[3].1.clone()).await;
    Json(json!({ "updatedAt": now_iso(), "items": ingest }))
}

async fn get_stream_ingest(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let publish = ensure_stream_tokens(&state).await;
    let srt_url = format!(
        "srt://127.0.0.1:8890?streamid=publish:{}:token:{}",
        state.mediamtx_path, publish
    );
    Json(json!({
        "srt": srt_url,
        "token": publish
    }))
}

async fn get_stream_play(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let read = ensure_read_token(&state).await;
    let webrtc_base = public_media_base(&state.mediamtx_webrtc, headers.get("host"));
    let hls_base = public_media_base(&state.mediamtx_hls, headers.get("host"));
    let whep = format!(
        "{}/{}/whep?token={}",
        webrtc_base.trim_end_matches('/'),
        state.mediamtx_path,
        read
    );
    let hls = format!(
        "{}/{}/index.m3u8?token={}",
        hls_base.trim_end_matches('/'),
        state.mediamtx_path,
        read
    );
    Json(json!({
        "whep": whep,
        "hls": hls,
        "token": read
    }))
}

fn public_media_base(base: &str, host_header: Option<&HeaderValue>) -> String {
    let Some(host_header) = host_header.and_then(|v| v.to_str().ok()) else {
        return base.to_string();
    };
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let (base_host_port, base_path) = rest.split_once('/').unwrap_or((rest, ""));
    let (base_host, base_port) = split_host_port(base_host_port);
    if !is_local_host(&base_host) {
        return base.to_string();
    }
    let (host_only, _host_port) = split_host_port(host_header);
    if is_local_host(&host_only) {
        return base.to_string();
    }
    let rebuilt_host = match base_port {
        Some(port) => format!("{host_only}:{port}"),
        None => host_only,
    };
    if base_path.is_empty() {
        format!("{scheme}://{rebuilt_host}")
    } else {
        format!("{scheme}://{rebuilt_host}/{base_path}")
    }
}

fn split_host_port(input: &str) -> (String, Option<String>) {
    let trimmed = input.trim();
    if trimmed.starts_with('[') {
        if let Some(end) = trimmed.find(']') {
            let host = trimmed[..=end].to_string();
            let rest = &trimmed[end + 1..];
            if let Some(port) = rest.strip_prefix(':') {
                return (host, Some(port.to_string()));
            }
            return (host, None);
        }
    }
    let mut parts = trimmed.rsplitn(2, ':');
    let last = parts.next().unwrap_or("");
    let head = parts.next();
    if let Some(head) = head {
        if !last.is_empty() {
            return (head.to_string(), Some(last.to_string()));
        }
    }
    (trimmed.to_string(), None)
}

fn is_local_host(host: &str) -> bool {
    let normalized = host.trim().trim_start_matches('[').trim_end_matches(']').to_lowercase();
    matches!(
        normalized.as_str(),
        "127.0.0.1" | "localhost" | "::1" | "0.0.0.0"
    )
}

async fn get_schedule(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let schedule = get_json(&state.pool, "schedule", default_kv()[4].1.clone()).await;
    Json(json!({ "updatedAt": now_iso(), "items": schedule }))
}

async fn get_channels(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let channels = get_json(&state.pool, "channels", default_kv()[5].1.clone()).await;
    Json(json!({ "updatedAt": now_iso(), "items": channels }))
}

async fn get_ops(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let ops = get_json(&state.pool, "ops_alerts", default_kv()[6].1.clone()).await;
    Json(json!({ "updatedAt": now_iso(), "items": ops }))
}

async fn get_nodes(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let nodes = get_json(&state.pool, "nodes", default_kv()[7].1.clone()).await;
    Json(json!({ "updatedAt": now_iso(), "items": nodes }))
}

async fn get_chat_latest(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, ChatMessage>(
        r#"
        SELECT id, user, text, DATE_FORMAT(created_at, '%Y-%m-%d %H:%i:%s') as created_at
        FROM chat_messages
        ORDER BY id DESC
        LIMIT 50
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    Json(json!({ "items": rows.into_iter().rev().collect::<Vec<_>>() }))
}

async fn post_chat_send(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ChatSend>,
) -> impl IntoResponse {
    let text = payload.text.trim();
    if text.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "empty" })));
    }

    let Some(viewer) = viewer_from_header(&headers, &state).await else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "login_required" })));
    };
    let user = viewer;

    let user = user.chars().take(24).collect::<String>();
    let text = text.chars().take(120).collect::<String>();

    let result = sqlx::query(
        "INSERT INTO chat_messages (user, text) VALUES (?, ?)"
    )
    .bind(&user)
    .bind(&text)
    .execute(&state.pool)
    .await;

    if result.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "db" })));
    }

    let msg = ChatMessage {
        id: result.unwrap().last_insert_id() as i64,
        user,
        text,
        created_at: now_iso(),
    };

    broadcast(&state, "chat:new", &json!(msg));

    if let Ok(count) = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM chat_messages WHERE created_at >= (NOW() - INTERVAL 60 SECOND)"
    )
    .fetch_one(&state.pool)
    .await
    {
        let patch = json!({ "chatRate": format!("{count} / min") });
        update_stream_patch(&state, &patch).await;
    }

    (StatusCode::OK, Json(json!({ "ok": true, "item": msg })))
}

async fn post_stream_telemetry(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<TelemetryPayload>,
) -> impl IntoResponse {
    let mut patch = json!({});
    if let Some(ms) = payload.e2e_latency_ms {
        if let Some(obj) = patch.as_object_mut() {
            obj.insert("latency".to_string(), json!(format!("{:.0}ms", ms)));
        }
    }
    if let Some(ms) = payload.playout_delay_ms {
        if let Some(obj) = patch.as_object_mut() {
            obj.insert("playoutDelay".to_string(), json!(format!("{:.0}ms", ms)));
        }
    }
    if let Some(ms) = payload.push_latency_ms {
        if let Some(obj) = patch.as_object_mut() {
            obj.insert("pushLatency".to_string(), json!(format!("{:.0}ms", ms)));
        }
    }
    if patch.as_object().map(|obj| obj.is_empty()).unwrap_or(true) {
        return (StatusCode::OK, Json(json!({ "ok": true })));
    }
    update_stream_patch(&state, &patch).await;
    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn mediamtx_auth(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AuthRequest>,
) -> impl IntoResponse {
    info!(
        "mediamtx_auth action={} path={:?} token_field={:?} query={:?} user={:?} pass={:?}",
        payload.action,
        payload.path,
        payload.token,
        payload.query,
        payload.user,
        payload.password
    );
    let action = payload.action.as_str();
    let mut path = payload.path.unwrap_or_default();
    if let Some(stripped) = path.strip_prefix('/') {
        path = stripped.to_string();
    }
    if let Some((base, _)) = path.split_once('?') {
        path = base.to_string();
    }
    if path != state.mediamtx_path {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "path" })));
    }

    let token_from_query = payload
        .query
        .as_deref()
        .and_then(|q| q.split('&').find_map(|kv| {
            let mut parts = kv.split('=');
            let key = parts.next()?;
            let value = parts.next()?;
            if key == "token" { Some(value.to_string()) } else { None }
        }));
    let clean_opt = |val: Option<String>| {
        val.and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        })
    };
    let user = clean_opt(payload.user.clone());
    let pass = clean_opt(payload.password.clone());
    let token_field = clean_opt(payload.token);
    let token = match (user, pass) {
        (Some(user), Some(pass)) if user == "token" => Some(pass),
        (Some(user), None) => Some(user),
        (Some(user), Some(_)) => Some(user),
        (None, Some(pass)) => Some(pass),
        _ => None,
    }
    .or(token_field)
    .or(token_from_query);

    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "token" })));
    };

    let (publish_token, read_token) = {
        let p = state.publish_token.read().await.clone();
        let r = state.read_token.read().await.clone();
        (p, r)
    };

    let allowed = match action {
        "publish" => token == publish_token,
        "read" | "playback" => token == read_token || token == publish_token,
        "api" | "metrics" | "pprof" => true,
        _ => false,
    };

    if allowed {
        (StatusCode::OK, Json(json!({ "ok": true })))
    } else {
        (StatusCode::UNAUTHORIZED, Json(json!({ "error": "denied" })))
    }
}

async fn get_mediamtx_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let client = Client::new();
    let url = format!("{}/metrics", state.mediamtx_metrics.trim_end_matches('/'));
    let resp = client.get(&url).send().await;
    let Ok(resp) = resp else { return Json(json!({ "error": "fetch" })) };
    let Ok(body) = resp.text().await else { return Json(json!({ "error": "read" })) };

    let mut stats = json!({});
    for line in body.lines() {
        if line.starts_with('#') { continue; }
        if line.contains("mediamtx_path_readers") || line.contains("mediamtx_path_bytes") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 2 {
                stats[parts[0]] = json!(parts[1]);
            }
        }
    }
    Json(json!({ "items": stats }))
}

async fn fetch_mediamtx_metrics(client: &Client, state: &AppState) -> Option<String> {
    let url = format!("{}/metrics", state.mediamtx_metrics.trim_end_matches('/'));
    let resp = client.get(&url).send().await.ok()?;
    resp.text().await.ok()
}

fn metric_value_for_path(body: &str, names: &[&str], path: &str) -> Option<f64> {
    for line in body.lines() {
        if line.starts_with('#') {
            continue;
        }
        for name in names {
            if !line.starts_with(name) {
                continue;
            }
            let value = line.split_whitespace().last()?.parse::<f64>().ok()?;
            if let Some(labels) = extract_labels(line) {
                if let Some(label_path) = label_value(&labels, "path")
                    .or_else(|| label_value(&labels, "name"))
                {
                    if label_path == path {
                        return Some(value);
                    }
                    continue;
                }
            }
            return Some(value);
        }
    }
    None
}

fn extract_labels(line: &str) -> Option<String> {
    let start = line.find('{')?;
    let end = line.find('}')?;
    if end <= start {
        return None;
    }
    Some(line[start + 1..end].to_string())
}

fn label_value(labels: &str, key: &str) -> Option<String> {
    for entry in labels.split(',') {
        let mut parts = entry.splitn(2, '=');
        let k = parts.next()?.trim();
        let v = parts.next()?.trim().trim_matches('"');
        if k == key {
            return Some(v.to_string());
        }
    }
    None
}

async fn compute_bitrate_mbps(
    state: &AppState,
    rx_bytes: Option<f64>,
    tx_bytes: Option<f64>,
) -> Option<f64> {
    let mut metrics = state.metrics_state.write().await;
    let now = Instant::now();
    let elapsed = now.duration_since(metrics.last_ts).as_secs_f64();
    if elapsed <= 0.1 {
        return None;
    }
    let mut bitrate = None;
    if let Some(rx) = rx_bytes {
        if let Some(prev) = metrics.last_rx_bytes {
            let delta = rx - prev;
            if delta >= 0.0 {
                bitrate = Some((delta * 8.0) / elapsed / 1_000_000.0);
            }
        }
        metrics.last_rx_bytes = Some(rx);
    } else if let Some(tx) = tx_bytes {
        if let Some(prev) = metrics.last_tx_bytes {
            let delta = tx - prev;
            if delta >= 0.0 {
                bitrate = Some((delta * 8.0) / elapsed / 1_000_000.0);
            }
        }
        metrics.last_tx_bytes = Some(tx);
    }
    metrics.last_ts = now;
    bitrate
}

async fn admin_login(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LoginPayload>,
) -> impl IntoResponse {
    if !verify_turnstile(&state, payload.turnstile_token.clone()).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "turnstile" })));
    }
    let authed = if payload.username == state.admin_user && payload.password == state.admin_pass {
        true
    } else {
        let accounts = get_admin_accounts(&state.pool).await;
        let username = payload.username.trim();
        let password_hash = hash_password(payload.password.trim());
        accounts.iter().any(|account| {
            (account.username.eq_ignore_ascii_case(username)
                || account.email.eq_ignore_ascii_case(username))
                && account.password_hash == password_hash
        })
    };
    if !authed {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid" })));
    }

    let token = generate_key();
    let mut tokens = state.tokens.write().await;
    tokens.insert(token.clone(), Instant::now() + state.token_ttl);

    (StatusCode::OK, Json(json!({ "token": token, "expiresIn": state.token_ttl.as_secs() })))
}

async fn admin_smtp_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let data = state.smtp.read().await.clone().map(|cfg| SmtpPublic {
        host: cfg.host,
        port: cfg.port,
        username: cfg.username,
        from: cfg.from,
        reply_to: cfg.reply_to,
        starttls: cfg.starttls,
        password_set: !cfg.password.is_empty(),
    });
    (StatusCode::OK, Json(json!({ "configured": data.is_some(), "data": data })))
}

async fn admin_smtp_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<SmtpConfig>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    if payload.host.trim().is_empty()
        || payload.username.trim().is_empty()
        || payload.from.trim().is_empty()
    {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" })));
    }

    let mut next = payload.clone();
    if next.password.trim().is_empty() {
        if let Some(existing) = &*state.smtp.read().await {
            next.password = existing.password.clone();
        }
    }

    *state.smtp.write().await = Some(next.clone());
    let payload = serde_json::to_value(&next).unwrap_or_else(|_| json!({}));
    set_json(&state.pool, "smtp_config", &payload).await;

    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn admin_smtp_test(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<SmtpTestPayload>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let to = payload.to.trim();
    if !is_valid_email(to) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "email" })));
    }
    let smtp = state.smtp.read().await.clone();
    let Some(smtp) = smtp else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "smtp_not_configured" })));
    };
    if let Err(err) = send_email_code_with_subject(
        &smtp,
        to,
        &generate_code(),
        "SMTP 测试邮件",
        "这是一封测试邮件，发送成功说明 SMTP 配置可用。\n\n时间：{code}",
    )
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": err })));
    }
    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn viewer_register_start(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RegisterStartPayload>,
) -> impl IntoResponse {
    if !verify_turnstile(&state, payload.turnstile_token.clone()).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "turnstile" })));
    }
    let email = payload.email.trim().to_lowercase();
    if !is_valid_email(&email) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "email" })));
    }
    let code = generate_code();
    let mut pending = state.pending_viewer_codes.write().await;
    pending.insert(
        email.clone(),
        PendingCode {
            code: code.clone(),
            expires_at: Instant::now() + state.email_code_ttl,
        },
    );
    if let Some(smtp) = &*state.smtp.read().await {
        if let Err(err) = send_email_code_with_subject(
            smtp,
            &email,
            &code,
            "观众账号验证码",
            "你的观众验证码是：{code}\n\n有效期约 10 分钟。\n如果不是你本人操作，请忽略。",
        )
        .await
        {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": err })));
        }
    } else if !state.email_echo {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "smtp_not_configured" })));
    }

    let mut response = json!({
        "ok": true,
        "expiresIn": state.email_code_ttl.as_secs()
    });
    if state.email_echo {
        if let Some(obj) = response.as_object_mut() {
            obj.insert("devCode".to_string(), json!(code));
        }
    }
    (StatusCode::OK, Json(response))
}

async fn viewer_register_verify(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RegisterVerifyPayload>,
) -> impl IntoResponse {
    if !verify_turnstile(&state, payload.turnstile_token.clone()).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "turnstile" })));
    }
    let email = payload.email.trim().to_lowercase();
    let username = payload.username.trim();
    let password = payload.password.trim();
    let code = payload.code.trim();

    if !is_valid_email(&email) || username.is_empty() || password.len() < 6 || code.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" })));
    }

    let mut pending = state.pending_viewer_codes.write().await;
    let Some(stored) = pending.get(&email) else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "code" })));
    };
    if stored.expires_at < Instant::now() || stored.code != code {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "code" })));
    }

    let mut accounts = get_viewer_accounts(&state.pool).await;
    if accounts.iter().any(|acc| acc.email.eq_ignore_ascii_case(&email)) {
        return (StatusCode::CONFLICT, Json(json!({ "error": "email_exists" })));
    }
    if accounts.iter().any(|acc| acc.username.eq_ignore_ascii_case(username)) {
        return (StatusCode::CONFLICT, Json(json!({ "error": "username_exists" })));
    }

    accounts.push(ViewerAccount {
        email: email.clone(),
        username: username.to_string(),
        password_hash: hash_password(password),
        created_at: now_iso(),
    });
    save_viewer_accounts(&state.pool, &accounts).await;
    pending.remove(&email);

    let token = generate_key();
    let mut tokens = state.viewer_tokens.write().await;
    tokens.insert(
        token.clone(),
        ViewerSession {
            username: username.to_string(),
            expires_at: Instant::now() + state.viewer_token_ttl,
        },
    );
    (StatusCode::OK, Json(json!({ "ok": true, "token": token, "username": username, "expiresIn": state.viewer_token_ttl.as_secs() })))
}

async fn viewer_login(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LoginPayload>,
) -> impl IntoResponse {
    if !verify_turnstile(&state, payload.turnstile_token.clone()).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "turnstile" })));
    }
    let accounts = get_viewer_accounts(&state.pool).await;
    let username = payload.username.trim();
    let password_hash = hash_password(payload.password.trim());
    let authed = accounts.iter().any(|account| {
        (account.username.eq_ignore_ascii_case(username)
            || account.email.eq_ignore_ascii_case(username))
            && account.password_hash == password_hash
    });
    if !authed {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid" })));
    }

    let token = generate_key();
    let mut tokens = state.viewer_tokens.write().await;
    tokens.insert(
        token.clone(),
        ViewerSession {
            username: username.to_string(),
            expires_at: Instant::now() + state.viewer_token_ttl,
        },
    );
    (StatusCode::OK, Json(json!({ "token": token, "username": username, "expiresIn": state.viewer_token_ttl.as_secs() })))
}

async fn viewer_me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let username = viewer_from_header(&headers, &state).await;
    let Some(username) = username else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid" })));
    };
    (StatusCode::OK, Json(json!({ "username": username })))
}

async fn viewer_profile_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let session = viewer_session_from_header(&headers, &state).await;
    let Some((_, username)) = session else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid" })));
    };
    let accounts = get_viewer_accounts(&state.pool).await;
    let Some(account) = accounts.iter().find(|acc| acc.username.eq_ignore_ascii_case(&username)) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" })));
    };
    (StatusCode::OK, Json(json!(ViewerProfile { username: account.username.clone(), email: account.email.clone() })))
}

async fn viewer_profile_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ViewerProfileUpdate>,
) -> impl IntoResponse {
    let session = viewer_session_from_header(&headers, &state).await;
    let Some((token, username)) = session else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid" })));
    };
    let next_username = payload.username.trim();
    let next_email = payload.email.trim().to_lowercase();
    if next_username.is_empty() || !is_valid_email(&next_email) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" })));
    }
    let mut accounts = get_viewer_accounts(&state.pool).await;
    let Some(current_idx) = accounts.iter().position(|acc| acc.username.eq_ignore_ascii_case(&username)) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" })));
    };
    if accounts.iter().any(|acc| acc.username.eq_ignore_ascii_case(next_username) && !acc.username.eq_ignore_ascii_case(&username)) {
        return (StatusCode::CONFLICT, Json(json!({ "error": "username_exists" })));
    }
    if accounts.iter().any(|acc| acc.email.eq_ignore_ascii_case(&next_email) && !acc.username.eq_ignore_ascii_case(&username)) {
        return (StatusCode::CONFLICT, Json(json!({ "error": "email_exists" })));
    }

    accounts[current_idx].username = next_username.to_string();
    accounts[current_idx].email = next_email.clone();
    save_viewer_accounts(&state.pool, &accounts).await;

    let mut tokens = state.viewer_tokens.write().await;
    if let Some(session) = tokens.get_mut(&token) {
        session.username = next_username.to_string();
    }

    (StatusCode::OK, Json(json!(ViewerProfile { username: next_username.to_string(), email: next_email })))
}

async fn admin_stream_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let mut base = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    merge_object(&mut base, &payload);
    if let Some(obj) = base.as_object_mut() {
        obj.insert("updatedAt".to_string(), json!(now_iso()));
    }
    set_json(&state.pool, "stream", &base).await;
    broadcast(&state, "stream:update", &base);
    (StatusCode::OK, Json(json!({ "ok": true, "data": base })))
}

async fn admin_stats_replace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let items = unwrap_items(&payload);
    set_json(&state.pool, "stats", &items).await;
    broadcast(&state, "stats:update", &items);
    (StatusCode::OK, Json(json!({ "ok": true, "items": items })))
}

async fn admin_health_replace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let items = unwrap_items(&payload);
    set_json(&state.pool, "health", &items).await;
    broadcast(&state, "health:update", &items);
    (StatusCode::OK, Json(json!({ "ok": true, "items": items })))
}

async fn admin_schedule_replace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let items = unwrap_items(&payload);
    set_json(&state.pool, "schedule", &items).await;
    broadcast(&state, "schedule:update", &items);
    (StatusCode::OK, Json(json!({ "ok": true, "items": items })))
}

async fn admin_channels_replace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let items = unwrap_items(&payload);
    set_json(&state.pool, "channels", &items).await;
    broadcast(&state, "channels:update", &items);
    (StatusCode::OK, Json(json!({ "ok": true, "items": items })))
}

async fn admin_ops_replace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let items = unwrap_items(&payload);
    set_json(&state.pool, "ops_alerts", &items).await;
    broadcast(&state, "ops:update", &items);
    (StatusCode::OK, Json(json!({ "ok": true, "items": items })))
}

async fn admin_nodes_replace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let items = unwrap_items(&payload);
    set_json(&state.pool, "nodes", &items).await;
    broadcast(&state, "nodes:update", &items);
    (StatusCode::OK, Json(json!({ "ok": true, "items": items })))
}

async fn admin_ingest_replace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let items = unwrap_items(&payload);
    set_json(&state.pool, "ingest_config", &items).await;
    broadcast(&state, "ingest:update", &items);
    (StatusCode::OK, Json(json!({ "ok": true, "items": items })))
}

async fn report_ingest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<StreamUpdate>,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }

    let mut current = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    if let Some(obj) = current.as_object_mut() {
        set_opt(obj, "roomId", payload.room_id);
        set_opt(obj, "title", payload.title);
        set_opt(obj, "host", payload.host);
        set_opt(obj, "resolution", payload.resolution);
        set_opt(obj, "status", payload.status);
        set_opt(obj, "bitrate", payload.bitrate);
        set_opt(obj, "latency", payload.latency);
        set_opt(obj, "chatRate", payload.chat_rate);
        set_opt(obj, "giftResponse", payload.gift_response);
        set_opt(obj, "giftLatency", payload.gift_latency);
        if let Some(viewers) = payload.viewers {
            obj.insert("viewers".to_string(), json!(viewers));
        }
        obj.insert("updatedAt".to_string(), json!(now_iso()));
    }
    set_json(&state.pool, "stream", &current).await;
    broadcast(&state, "stream:update", &current);
    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn admin_room_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_bearer(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }

    let key = generate_key();
    let room_id = format!("{}", chrono::Utc::now().timestamp());
    {
        let mut publish = state.publish_token.write().await;
        *publish = key.clone();
    }
    let read = generate_key();
    {
        let mut read_token = state.read_token.write().await;
        *read_token = read.clone();
    }

    let srt_url = format!(
        "srt://127.0.0.1:8890?streamid=publish:{}:token:{}",
        state.mediamtx_path,
        key
    );

    let mut stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    if let Some(obj) = stream.as_object_mut() {
        obj.insert("roomId".to_string(), json!(room_id));
        obj.insert("status".to_string(), json!("ready"));
        obj.insert("updatedAt".to_string(), json!(now_iso()));
    }
    set_json(&state.pool, "stream", &stream).await;
    broadcast(&state, "stream:update", &stream);

    let ingest = json!([
        {
            "protocol": "SRT",
            "url": srt_url,
            "note": "本地测试线路（监听端口 9000）",
            "tag": "本地测试"
        },
        {
            "protocol": "OBS",
            "url": "obs://profile/live",
            "note": "编码器：H.264 / 1080p60",
            "tag": "推流配置"
        },
        {
            "label": "Key",
            "value": key,
            "note": "用于本地测试",
            "tag": "已生成"
        }
    ]);
    set_json(&state.pool, "ingest_config", &ingest).await;
    broadcast(&state, "ingest:update", &ingest);

    (StatusCode::OK, Json(json!({ "ok": true, "roomId": room_id, "key": key, "srtUrl": srt_url, "readToken": read })))
}

async fn ws_handler(
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.broadcaster.subscribe();
    let snapshot = json!({
        "stream": get_json(&state.pool, "stream", default_kv()[0].1.clone()).await,
        "stats": get_json(&state.pool, "stats", default_kv()[1].1.clone()).await,
        "health": get_json(&state.pool, "health", default_kv()[2].1.clone()).await,
        "ingestConfig": get_json(&state.pool, "ingest_config", default_kv()[3].1.clone()).await,
        "schedule": get_json(&state.pool, "schedule", default_kv()[4].1.clone()).await,
        "channels": get_json(&state.pool, "channels", default_kv()[5].1.clone()).await,
        "opsAlerts": get_json(&state.pool, "ops_alerts", default_kv()[6].1.clone()).await,
        "nodes": get_json(&state.pool, "nodes", default_kv()[7].1.clone()).await,
        "chat": {
            "items": sqlx::query_as::<_, ChatMessage>(
                r#"
                SELECT id, user, text, DATE_FORMAT(created_at, '%Y-%m-%d %H:%i:%s') as created_at
                FROM chat_messages
                ORDER BY id DESC
                LIMIT 50
                "#
            )
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
        }
    });

    if socket
        .send(WsMessage::Text(
            json!({ "type": "snapshot", "data": snapshot }).to_string(),
        ))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(text) => {
                        if socket.send(WsMessage::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            incoming = socket.recv() => {
                if incoming.is_none() {
                    break;
                }
            }
        }
    }
}

fn broadcast(state: &AppState, event: &str, data: &Value) {
    let payload = json!({ "type": event, "data": data });
    let _ = state.broadcaster.send(payload.to_string());
}

async fn check_bearer(headers: &HeaderMap, state: &AppState) -> bool {
    let token = headers
        .get("authorization")
        .and_then(|val| val.to_str().ok())
        .and_then(|val| val.strip_prefix("Bearer "))
        .map(|val| val.to_string());

    let Some(token) = token else { return false };

    let mut tokens = state.tokens.write().await;
    let now = Instant::now();
    tokens.retain(|_, exp| *exp > now);
    tokens.get(&token).is_some()
}

async fn viewer_from_header(headers: &HeaderMap, state: &AppState) -> Option<String> {
    let token = headers
        .get("authorization")
        .and_then(|val| val.to_str().ok())
        .and_then(|val| val.strip_prefix("Bearer "))
        .map(|val| val.to_string());

    let Some(token) = token else { return None };

    let mut tokens = state.viewer_tokens.write().await;
    let now = Instant::now();
    tokens.retain(|_, session| session.expires_at > now);
    tokens.get(&token).map(|session| session.username.clone())
}

async fn viewer_session_from_header(headers: &HeaderMap, state: &AppState) -> Option<(String, String)> {
    let token = headers
        .get("authorization")
        .and_then(|val| val.to_str().ok())
        .and_then(|val| val.strip_prefix("Bearer "))
        .map(|val| val.to_string());

    let Some(token) = token else { return None };

    let mut tokens = state.viewer_tokens.write().await;
    let now = Instant::now();
    tokens.retain(|_, session| session.expires_at > now);
    tokens.get(&token).map(|session| (token.clone(), session.username.clone()))
}

async fn get_admin_accounts(pool: &MySqlPool) -> Vec<AdminAccount> {
    let raw = get_json(pool, "admin_accounts", json!([])).await;
    serde_json::from_value(raw).unwrap_or_default()
}


async fn get_viewer_accounts(pool: &MySqlPool) -> Vec<ViewerAccount> {
    let raw = get_json(pool, "viewer_accounts", json!([])).await;
    serde_json::from_value(raw).unwrap_or_default()
}

async fn save_viewer_accounts(pool: &MySqlPool, accounts: &Vec<ViewerAccount>) {
    let payload = serde_json::to_value(accounts).unwrap_or_else(|_| json!([]));
    set_json(pool, "viewer_accounts", &payload).await;
}

fn hash_password(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn is_valid_email(email: &str) -> bool {
    let email = email.trim();
    if email.len() < 5 || !email.contains('@') {
        return false;
    }
    let parts: Vec<&str> = email.split('@').collect();
    parts.len() == 2 && parts[1].contains('.')
}

fn generate_code() -> String {
    format!("{:06}", rand_u64() % 1_000_000)
}

fn load_smtp_config() -> Option<SmtpConfig> {
    let host = std::env::var("SMTP_HOST").ok()?;
    let port = std::env::var("SMTP_PORT")
        .ok()
        .and_then(|val| val.parse::<u16>().ok())
        .unwrap_or(587);
    let username = std::env::var("SMTP_USER").ok()?;
    let password = std::env::var("SMTP_PASS").ok()?;
    let from = std::env::var("SMTP_FROM").ok()?;
    let reply_to = std::env::var("SMTP_REPLY_TO").ok().filter(|v| !v.trim().is_empty());
    let starttls = std::env::var("SMTP_STARTTLS")
        .map(|val| val.to_lowercase() != "false")
        .unwrap_or(true);
    Some(SmtpConfig {
        host,
        port,
        username,
        password,
        from,
        reply_to,
        starttls,
    })
}

async fn load_smtp_config_db(pool: &MySqlPool) -> Option<SmtpConfig> {
    let raw = get_json(pool, "smtp_config", json!({})).await;
    serde_json::from_value(raw).ok()
}

async fn send_email_code_with_subject(
    config: &SmtpConfig,
    to: &str,
    code: &str,
    subject: &str,
    body_tpl: &str,
) -> Result<(), String> {
    let mut builder = MailMessage::builder()
        .from(config.from.parse().map_err(|_| "smtp_from_invalid")?)
        .to(to.parse().map_err(|_| "smtp_to_invalid")?)
        .subject(subject);
    if let Some(reply_to) = &config.reply_to {
        builder = builder.reply_to(reply_to.parse().map_err(|_| "smtp_reply_invalid")?);
    }
    let body = body_tpl.replace("{code}", code);
    let email = builder.body(body).map_err(|_| "smtp_build_failed")?;

    let creds = Credentials::new(config.username.clone(), config.password.clone());
    let mailer = if config.starttls {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
            .map_err(|_| "smtp_starttls_failed")?
            .port(config.port)
            .credentials(creds)
            .build()
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
            .map_err(|_| "smtp_relay_failed")?
            .port(config.port)
            .credentials(creds)
            .build()
    };

    mailer.send(email).await.map_err(|_| "smtp_send_failed")?;
    Ok(())
}

fn merge_object(target: &mut Value, patch: &Value) {
    if let (Some(target_obj), Some(patch_obj)) = (target.as_object_mut(), patch.as_object()) {
        for (k, v) in patch_obj {
            target_obj.insert(k.clone(), v.clone());
        }
    }
}

async fn update_stream_patch(state: &AppState, patch: &Value) {
    let mut current = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    merge_object(&mut current, patch);
    if let Some(obj) = current.as_object_mut() {
        obj.insert("updatedAt".to_string(), json!(now_iso()));
    }
    set_json(&state.pool, "stream", &current).await;
    broadcast(state, "stream:update", &current);
}

fn unwrap_items(payload: &Value) -> Value {
    payload
        .get("items")
        .cloned()
        .unwrap_or_else(|| payload.clone())
}

fn set_opt(obj: &mut serde_json::Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        obj.insert(key.to_string(), json!(value));
    }
}

async fn verify_turnstile(state: &AppState, token: Option<String>) -> bool {
    let Some(secret) = &state.turnstile_secret else {
        return true;
    };
    let Some(token) = token.filter(|val| !val.trim().is_empty()) else {
        return false;
    };
    let client = Client::new();
    let res = client
        .post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .form(&[("secret", secret.as_str()), ("response", token.as_str())])
        .send()
        .await;

    let Ok(res) = res else { return false };
    let Ok(payload) = res.json::<Value>().await else { return false };
    payload.get("success").and_then(|v| v.as_bool()).unwrap_or(false)
}

async fn run_srt_listener(
    state: &Arc<AppState>,
    listen: &str,
    forward: Option<&str>,
) -> Result<(), String> {
    info!("SRT listening on {listen}");
    loop {
        let socket = match SrtSocket::builder().listen_on(listen).await {
            Ok(socket) => socket,
            Err(err) => {
                return Err(format!("SRT bind failed: {err}"));
            }
        };
        let forward_target = forward.map(|val| val.to_string());
        let state = state.clone();
        if let Err(err) = handle_srt_session(state, socket, forward_target).await {
            info!("SRT session ended: {err}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn handle_srt_session(
    state: Arc<AppState>,
    mut inbound: SrtSocket,
    forward: Option<String>,
) -> Result<(), String> {
    state.active_streams.fetch_add(1, Ordering::SeqCst);
    let _guard = StreamGuard { state: state.clone() };
    update_stream_status(&state, "live").await;

    let mut outbound = if let Some(target) = forward {
        Some(
            SrtSocket::builder()
                .call(target.as_str(), None)
                .await
                .map_err(|err| format!("SRT connect failed: {err}"))?,
        )
    } else {
        None
    };

    let mut bytes_total: u64 = 0;
    let mut last_tick = Instant::now();
    let mut last_bytes = 0u64;

    while let Some(frame) = inbound.next().await {
        let (instant, data) = frame.map_err(|err| format!("SRT read error: {err}"))?;
        bytes_total += data.len() as u64;

        if let Some(out) = outbound.as_mut() {
            out.send((instant, data))
                .await
                .map_err(|err| format!("SRT forward write error: {err}"))?;
        }

        if last_tick.elapsed() >= Duration::from_secs(2) {
            let bytes_diff = bytes_total.saturating_sub(last_bytes);
            let seconds = last_tick.elapsed().as_secs_f64().max(0.001);
            let mbps = (bytes_diff as f64 * 8.0) / seconds / 1_000_000.0;
            let bitrate = format!("{mbps:.2} Mbps");
            update_stream_metric(&state, "bitrate", json!(bitrate)).await;
            update_stream_metric(&state, "status", json!("live")).await;
            last_tick = Instant::now();
            last_bytes = bytes_total;
        }
    }

    Ok(())
}

struct StreamGuard {
    state: Arc<AppState>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if self.state.active_streams.fetch_sub(1, Ordering::SeqCst) == 1 {
            let state = self.state.clone();
            tokio::spawn(async move {
                update_stream_status(&state, "offline").await;
            });
        }
    }
}

async fn update_stream_status(state: &AppState, status: &str) {
    let mut current = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    if let Some(obj) = current.as_object_mut() {
        obj.insert("status".to_string(), json!(status));
        obj.insert("updatedAt".to_string(), json!(now_iso()));
    }
    set_json(&state.pool, "stream", &current).await;
    broadcast(state, "stream:update", &current);
}

async fn update_stream_metric(state: &AppState, key: &str, value: Value) {
    let mut current = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    if let Some(obj) = current.as_object_mut() {
        obj.insert(key.to_string(), value);
        obj.insert("updatedAt".to_string(), json!(now_iso()));
    }
    set_json(&state.pool, "stream", &current).await;
    broadcast(state, "stream:update", &current);
}

async fn update_stream_fields(
    state: &AppState,
    status: String,
    viewers: Option<f64>,
    bitrate: Option<String>,
    push_latency: Option<String>,
) {
    let mut patch = json!({
        "status": status,
        "updatedAt": now_iso()
    });
    if let Some(viewers) = viewers {
        if let Some(obj) = patch.as_object_mut() {
            obj.insert("viewers".to_string(), json!(viewers));
        }
    }
    if let Some(bitrate) = bitrate {
        if let Some(obj) = patch.as_object_mut() {
            obj.insert("bitrate".to_string(), json!(bitrate));
        }
    }
    if let Some(push_latency) = push_latency {
        if let Some(obj) = patch.as_object_mut() {
            obj.insert("pushLatency".to_string(), json!(push_latency));
        }
    }
    update_stream_patch(state, &patch).await;
}

fn generate_key() -> String {
    let mut key = String::from("meow_live_");
    key.push_str(&format!("{:x}", rand_u64()));
    key
}

fn rand_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ 0x5a5a5a5a5a5a5a5a
}

async fn ensure_stream_tokens(state: &Arc<AppState>) -> String {
    {
        let publish = state.publish_token.read().await;
        if !publish.is_empty() {
            return publish.clone();
        }
    }
    let token = generate_key();
    let mut publish = state.publish_token.write().await;
    *publish = token.clone();
    token
}

async fn ensure_read_token(state: &Arc<AppState>) -> String {
    {
        let read = state.read_token.read().await;
        if !read.is_empty() {
            return read.clone();
        }
    }
    let token = generate_key();
    let mut read = state.read_token.write().await;
    *read = token.clone();
    token
}
