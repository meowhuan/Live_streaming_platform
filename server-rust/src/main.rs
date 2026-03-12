use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use axum::http::header::SET_COOKIE;
use dotenvy::dotenv;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use futures::{SinkExt, StreamExt};
use srt_tokio::SrtSocket;
use sqlx::{mysql::MySqlPoolOptions, MySqlPool};
use reqwest::Client;
use sha2::{Digest, Sha256};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message as MailMessage,
    message::header::ContentType,
    transport::smtp::authentication::Credentials,
    Tokio1Executor,
};
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tokio::process::Command;
use tokio::fs;
use tower_http::cors::CorsLayer;
use chrono::TimeZone;
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
    viewer_anti_abuse_defaults: ViewerAntiAbuseConfig,
    cf_access_team_domain: Option<String>,
    cf_access_aud: Option<String>,
    cf_access_jwks: RwLock<Option<CfAccessJwksCache>>,
    telegram_bot_token: Option<String>,
    chat_filter_words: Vec<String>,
    public_base_url: String,
    stats_last_written: RwLock<Instant>,
    replay_max_seconds: u64,
    hls_segment_duration_secs: u64,
    hls_segment_count: u64,
    clip_min_secs: u64,
    clip_max_secs: u64,
    clip_dir: String,
    ffmpeg_bin: String,
    mediamtx_ll_hls: Option<String>,
    thumbnail_interval: Duration,
    thumbnail_source: Option<String>,
}

const ADMIN_COOKIE: &str = "meow_admin_token";
const VIEWER_COOKIE: &str = "meow_viewer_token";

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
    #[serde(rename = "rememberDays")]
    remember_days: Option<u64>,
}

#[derive(Deserialize)]
struct RegisterStartPayload {
    email: String,
    #[serde(rename = "registerToken")]
    register_token: Option<String>,
}

#[derive(Deserialize)]
struct RegisterTokenPayload {
    #[serde(rename = "turnstileToken")]
    turnstile_token: Option<String>,
}

#[derive(Deserialize)]
struct AdminTurnstilePayload {
    #[serde(rename = "turnstileToken")]
    turnstile_token: Option<String>,
    #[serde(rename = "rememberDays")]
    remember_days: Option<u64>,
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
    is_host: bool,
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

#[derive(Deserialize)]
struct TelegramChannelPayload {
    channel: String,
}

#[derive(Serialize)]
struct TelegramChannelPublic {
    channel: String,
    token_configured: bool,
}

#[derive(Serialize, Deserialize, Clone)]
struct ViewerAntiAbuseConfig {
    verify_email_rate_limit_window_secs: i64,
    verify_email_rate_limit_max: i64,
    verify_email_rate_limit_email_max: i64,
    verify_email_cooldown_secs: i64,
    block_disposable_email: bool,
    block_edu_gov_email: bool,
    register_token_ttl_secs: i64,
}

#[derive(Deserialize)]
struct ViewerAntiAbusePayload {
    verify_email_rate_limit_window_secs: Option<i64>,
    verify_email_rate_limit_max: Option<i64>,
    verify_email_rate_limit_email_max: Option<i64>,
    verify_email_cooldown_secs: Option<i64>,
    block_disposable_email: Option<bool>,
    block_edu_gov_email: Option<bool>,
    register_token_ttl_secs: Option<i64>,
}

#[derive(Serialize)]
struct ViewerAntiAbusePublic {
    verify_email_rate_limit_window_secs: i64,
    verify_email_rate_limit_max: i64,
    verify_email_rate_limit_email_max: i64,
    verify_email_cooldown_secs: i64,
    block_disposable_email: bool,
    block_edu_gov_email: bool,
    register_token_ttl_secs: i64,
}

impl From<&ViewerAntiAbuseConfig> for ViewerAntiAbusePublic {
    fn from(cfg: &ViewerAntiAbuseConfig) -> Self {
        Self {
            verify_email_rate_limit_window_secs: cfg.verify_email_rate_limit_window_secs,
            verify_email_rate_limit_max: cfg.verify_email_rate_limit_max,
            verify_email_rate_limit_email_max: cfg.verify_email_rate_limit_email_max,
            verify_email_cooldown_secs: cfg.verify_email_cooldown_secs,
            block_disposable_email: cfg.block_disposable_email,
            block_edu_gov_email: cfg.block_edu_gov_email,
            register_token_ttl_secs: cfg.register_token_ttl_secs,
        }
    }
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

#[allow(dead_code)]
#[derive(Deserialize)]
struct CfAccessClaims {
    aud: Vec<String>,
    exp: usize,
    iat: usize,
    sub: String,
    email: Option<String>,
}

#[derive(Deserialize, Clone)]
struct CfAccessJwks {
    keys: Vec<CfAccessJwk>,
}

#[derive(Deserialize, Clone)]
struct CfAccessJwk {
    kid: String,
    #[allow(dead_code)]
    kty: Option<String>,
    n: String,
    e: String,
}

struct CfAccessJwksCache {
    fetched_at: Instant,
    keys: Vec<CfAccessJwk>,
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
    fps: Option<f64>,
}

#[derive(sqlx::FromRow)]
struct ChatMessageRow {
    id: i64,
    user: String,
    text: String,
    created_at: String,
    msg_count: i64,
}

#[derive(Serialize)]
struct ChatMessage {
    id: i64,
    user: String,
    text: String,
    created_at: String,
    msg_count: i64,
    level: u32,
    level_label: String,
}

#[derive(Deserialize)]
struct ClipCreatePayload {
    #[serde(rename = "lengthSecs")]
    length_secs: Option<u64>,
}

#[derive(Deserialize)]
struct LeaderboardQuery {
    limit: Option<u64>,
}

#[derive(Deserialize)]
struct HistoryQuery {
    limit: Option<u64>,
    since: Option<i64>,
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
    let mediamtx_ll_hls = std::env::var("MEDIAMTX_LL_HLS").ok().filter(|v| !v.trim().is_empty());
    let mediamtx_poll = env_u64("MEDIAMTX_POLL_MS", 2000);
    let hls_segment_duration_secs = env_u64("HLS_SEGMENT_DURATION_SECS", 2);
    let hls_segment_count = env_u64("HLS_SEGMENT_COUNT", 60);
    let replay_max_seconds = env_u64("REPLAY_MAX_SECONDS", 120);
    let clip_min_secs = env_u64("CLIP_MIN_SECS", 15);
    let clip_max_secs = env_u64("CLIP_MAX_SECS", 30);
    let clip_dir = std::env::var("CLIP_DIR").unwrap_or_else(|_| "public/clips".to_string());
    let ffmpeg_bin = std::env::var("FFMPEG_BIN").unwrap_or_else(|_| "ffmpeg".to_string());
    let thumbnail_interval = Duration::from_secs(env_u64("THUMBNAIL_INTERVAL_SECS", 30).max(10));
    let thumbnail_source = std::env::var("THUMBNAIL_SOURCE").ok().filter(|v| !v.trim().is_empty());
    let email_code_ttl = Duration::from_secs(env_u64("EMAIL_CODE_TTL_MIN", 10) * 60);
    let email_echo = std::env::var("EMAIL_ECHO")
        .map(|val| val.to_lowercase() != "false")
        .unwrap_or(true);
    let public_base_url = std::env::var("PUBLIC_BASE_URL").unwrap_or_default();
    let turnstile_secret = std::env::var("TURNSTILE_SECRET").ok().filter(|val| !val.trim().is_empty());
    let cf_access_team_domain = std::env::var("CF_ACCESS_TEAM_DOMAIN")
        .ok()
        .filter(|val| !val.trim().is_empty());
    let cf_access_aud = std::env::var("CF_ACCESS_AUD")
        .or_else(|_| std::env::var("CF_ACCESS_AUDIENCE"))
        .ok()
        .filter(|val| !val.trim().is_empty());
    let telegram_bot_token = std::env::var("TELEGRAM_BOT_TOKEN").ok().filter(|v| !v.trim().is_empty());
    let chat_filter_words = std::env::var("CHAT_FILTER_WORDS")
        .ok()
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let viewer_token_ttl = Duration::from_secs(env_u64("VIEWER_TOKEN_DAYS", 30) * 24 * 3600);
    let viewer_anti_abuse_defaults = ViewerAntiAbuseConfig::from_env();

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
        viewer_anti_abuse_defaults,
        cf_access_team_domain,
        cf_access_aud,
        cf_access_jwks: RwLock::new(None),
        telegram_bot_token,
        chat_filter_words,
        public_base_url,
        stats_last_written: RwLock::new(Instant::now() - Duration::from_secs(3600)),
        replay_max_seconds,
        hls_segment_duration_secs,
        hls_segment_count,
        clip_min_secs,
        clip_max_secs,
        clip_dir,
        ffmpeg_bin,
        mediamtx_ll_hls,
        thumbnail_interval,
        thumbnail_source,
    };

    {
        let publish_saved = get_json(&state.pool, "publish_token", json!("")).await;
        let read_saved = get_json(&state.pool, "read_token", json!("")).await;
        let mut publish = state.publish_token.write().await;
        let mut read = state.read_token.write().await;
        let publish_value = publish_saved.as_str().unwrap_or("").to_string();
        let read_value = read_saved.as_str().unwrap_or("").to_string();
        if publish_value.trim().is_empty() {
            let next = generate_key();
            *publish = next.clone();
            set_json(&state.pool, "publish_token", &json!(next)).await;
        } else {
            *publish = publish_value;
        }
        if read_value.trim().is_empty() {
            let next = generate_key();
            *read = next.clone();
            set_json(&state.pool, "read_token", &json!(next)).await;
        } else {
            *read = read_value;
        }
    }

    let router = Router::new()
        .route("/api/stream", get(get_stream))
        .route("/api/stream/status", get(get_stream_status))
        .route("/api/stream/stats", get(get_stats))
        .route("/api/stream/health", get(get_health))
        .route("/api/ingest/config", get(get_ingest))
        .route("/api/stream/ingest", get(get_stream_ingest))
        .route("/api/stream/play", get(get_stream_play))
        .route("/api/stream/replay", get(get_stream_replay))
        .route("/api/stream/telemetry", post(post_stream_telemetry))
        .route("/api/live/status", get(get_live_status))
        .route("/api/live/stats", get(get_live_stats))
        .route("/api/live/history", get(get_live_history))
        .route("/api/live/countdown", get(get_live_countdown))
        .route("/api/schedule", get(get_schedule))
        .route("/api/channels", get(get_channels))
        .route("/api/ops/alerts", get(get_ops))
        .route("/api/ingest/nodes", get(get_nodes))
        .route("/api/chat/latest", get(get_chat_latest))
        .route("/api/chat/activity", get(get_chat_activity))
        .route("/api/chat/leaderboard", get(get_chat_leaderboard))
        .route("/api/chat/send", post(post_chat_send))
        .route("/api/clip/create", post(post_clip_create))
        .route("/api/mediamtx/auth", post(mediamtx_auth))
        .route("/api/mediamtx/metrics", get(get_mediamtx_metrics))
        .route("/api/notifications", get(get_notifications))
        .route("/api/admin/access-mode", get(admin_access_mode))
        .route("/api/admin/login", post(admin_login))
        .route("/api/admin/logout", post(admin_logout))
        .route("/api/admin/me", get(admin_me))
        .route("/api/admin/turnstile-login", post(admin_turnstile_login))
        .route("/api/admin/smtp", get(admin_smtp_get))
        .route("/api/admin/smtp", post(admin_smtp_set))
        .route("/api/admin/smtp/test", post(admin_smtp_test))
        .route("/api/admin/telegram/channel", get(admin_telegram_get))
        .route("/api/admin/telegram/channel", post(admin_telegram_set))
        .route("/api/admin/notify-templates", get(admin_notify_templates_get))
        .route("/api/admin/notify-templates", post(admin_notify_templates_set))
        .route("/api/admin/viewer-anti-abuse", get(admin_viewer_anti_abuse_get))
        .route("/api/admin/viewer-anti-abuse", post(admin_viewer_anti_abuse_set))
        .route("/api/viewer/login", post(viewer_login))
        .route("/api/viewer/logout", post(viewer_logout))
        .route("/api/viewer/subscribe", get(viewer_subscription_get))
        .route("/api/viewer/subscribe", post(viewer_subscription_set))
        .route("/api/viewer/register/token", post(viewer_register_token))
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
        .route("/api/admin/ingest/refresh", post(admin_ingest_refresh))
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

    let thumb_state = state_for_srt.clone();
    tokio::spawn(async move {
        run_thumbnail_loop(thumb_state).await;
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

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS viewer_email_log (
          id BIGINT PRIMARY KEY AUTO_INCREMENT,
          email VARCHAR(190) NOT NULL,
          ip VARCHAR(64) NOT NULL,
          created_at BIGINT NOT NULL,
          INDEX idx_viewer_email_created (email, created_at),
          INDEX idx_viewer_ip_created (ip, created_at)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
        "#,
    )
    .execute(pool)
    .await
    .expect("create viewer_email_log failed");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS viewer_register_tokens (
          token VARCHAR(64) PRIMARY KEY,
          ip VARCHAR(64) NOT NULL,
          created_at BIGINT NOT NULL,
          expires_at BIGINT NOT NULL,
          used_at BIGINT NULL,
          INDEX idx_viewer_register_ip_created (ip, created_at),
          INDEX idx_viewer_register_expires (expires_at)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
        "#,
    )
    .execute(pool)
    .await
    .expect("create viewer_register_tokens failed");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS clips (
          id VARCHAR(32) PRIMARY KEY,
          created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
          created_by VARCHAR(64) NOT NULL,
          length_secs INT NOT NULL,
          status VARCHAR(16) NOT NULL,
          url VARCHAR(255) NOT NULL,
          error TEXT NULL
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
        "#,
    )
    .execute(pool)
    .await
    .expect("create clips failed");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS stream_stats_history (
          id BIGINT PRIMARY KEY AUTO_INCREMENT,
          ts BIGINT NOT NULL,
          viewer_count INT NULL,
          bitrate_kbps INT NULL,
          fps INT NULL,
          latency_e2e_ms INT NULL,
          latency_publish_ms INT NULL,
          latency_playback_ms INT NULL,
          INDEX idx_stream_stats_ts (ts)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
        "#,
    )
    .execute(pool)
    .await
    .expect("create stream_stats_history failed");
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
        "latencyE2eMs": 0,
        "publishLatencyMs": 0,
        "playbackLatencyMs": 0,
        "fps": "60 fps",
        "fpsValue": 60,
        "chatRate": "132 / min",
        "giftResponse": "98.2%",
        "giftLatency": "0.4s",
        "viewers": 12842,
        "startedAt": 0,
        "endedAt": 0,
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

    let notifications = json!([]);
    let subscriptions = json!({});
    let telegram_channel = json!("");
    let notify_templates = default_notify_templates();

    vec![
        ("stream".to_string(), stream),
        ("stats".to_string(), stats),
        ("health".to_string(), health),
        ("ingest_config".to_string(), ingest),
        ("schedule".to_string(), schedule),
        ("channels".to_string(), channels),
        ("ops_alerts".to_string(), ops),
        ("nodes".to_string(), nodes),
        ("notifications".to_string(), notifications),
        ("viewer_subscriptions".to_string(), subscriptions),
        ("telegram_channel".to_string(), telegram_channel),
        ("notify_templates".to_string(), notify_templates),
    ]
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn parse_number(input: &str) -> Option<f64> {
    let mut buf = String::new();
    for ch in input.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            buf.push(ch);
        } else if !buf.is_empty() {
            break;
        }
    }
    buf.parse::<f64>().ok()
}

fn parse_duration_ms(input: &str) -> Option<i64> {
    let trimmed = input.trim().to_lowercase();
    if trimmed.ends_with("ms") {
        return parse_number(&trimmed).map(|v| v.round() as i64);
    }
    if trimmed.ends_with('s') {
        return parse_number(&trimmed).map(|v| (v * 1000.0) as i64);
    }
    parse_number(&trimmed).map(|v| v.round() as i64)
}

fn read_latency_ms(stream: &Value, numeric_key: &str, fallback_key: &str) -> i64 {
    if let Some(value) = stream.get(numeric_key).and_then(|v| v.as_i64()) {
        return value;
    }
    if let Some(raw) = stream.get(fallback_key).and_then(|v| v.as_str()) {
        return parse_duration_ms(raw).unwrap_or(0);
    }
    0
}

fn chat_level(count: i64) -> u32 {
    if count >= 1000 {
        4
    } else if count >= 200 {
        3
    } else if count >= 50 {
        2
    } else if count >= 10 {
        1
    } else {
        0
    }
}

fn chat_heat(count_10s: i64) -> &'static str {
    if count_10s >= 30 {
        "High"
    } else if count_10s >= 10 {
        "Medium"
    } else {
        "Low"
    }
}

fn replace_chat_emojis(input: &str) -> String {
    let mut out = input.to_string();
    let replacements = [
        (":cat:", "🐱"),
        (":awawa:", "🥺"),
        (":thonk:", "🤔"),
    ];
    for (key, value) in replacements {
        out = out.replace(key, value);
    }
    out
}

fn parse_schedule_start(item: &Value) -> Option<i64> {
    if let Some(ts) = item.get("startsAt").and_then(|v| v.as_i64()) {
        return Some(ts);
    }
    if let Some(raw) = item.get("startsAt").and_then(|v| v.as_str()) {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
            return Some(dt.timestamp());
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
            return Some(chrono::Utc.from_utc_datetime(&dt).timestamp());
        }
    }
    None
}

fn default_notify_templates() -> Value {
    json!({
        "live": {
            "title": "主播已开播",
            "message": "直播间已开始：{title}\n主播：{host}\n观看地址：{liveUrl}",
            "url": ""
        },
        "offline": {
            "title": "主播已下播",
            "message": "直播已结束：{title}\n感谢陪伴",
            "url": ""
        },
        "schedule": {
            "title": "排期已更新",
            "message": "今日排期已同步，下一场：{scheduleTime} {scheduleTitle}",
            "url": ""
        },
        "live_url": "",
        "rules": []
    })
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

fn env_i64(key: &str, fallback: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|val| val.parse::<i64>().ok())
        .unwrap_or(fallback)
}

fn env_bool(key: &str, fallback: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|val| matches!(val.as_str(), "1" | "true" | "TRUE" | "True"))
        .unwrap_or(fallback)
}

fn normalize_remember_days(days: Option<u64>) -> Option<u64> {
    match days {
        Some(7) => Some(7),
        Some(30) => Some(30),
        _ => None,
    }
}

fn ttl_from_remember(default: Duration, remember_days: Option<u64>) -> Duration {
    if let Some(days) = normalize_remember_days(remember_days) {
        return Duration::from_secs(days * 24 * 3600);
    }
    default
}

impl ViewerAntiAbuseConfig {
    fn from_env() -> Self {
        let verify_email_rate_limit_window_secs =
            env_i64("VIEWER_VERIFY_EMAIL_RATE_LIMIT_WINDOW_SEC", 1800).clamp(60, 86400);
        let verify_email_rate_limit_max =
            env_i64("VIEWER_VERIFY_EMAIL_RATE_LIMIT_MAX", 3).clamp(1, 20);
        let verify_email_rate_limit_email_max =
            env_i64("VIEWER_VERIFY_EMAIL_RATE_LIMIT_EMAIL_MAX", 2).clamp(1, 20);
        let verify_email_cooldown_secs =
            env_i64("VIEWER_VERIFY_EMAIL_COOLDOWN_SEC", 600).clamp(60, 7200);
        let block_disposable_email = env_bool("VIEWER_BLOCK_DISPOSABLE_EMAIL", true);
        let block_edu_gov_email = env_bool("VIEWER_BLOCK_EDU_GOV_EMAIL", true);
        let register_token_ttl_secs =
            env_i64("VIEWER_REGISTER_TOKEN_TTL_SEC", 600).clamp(60, 3600);

        Self {
            verify_email_rate_limit_window_secs,
            verify_email_rate_limit_max,
            verify_email_rate_limit_email_max,
            verify_email_cooldown_secs,
            block_disposable_email,
            block_edu_gov_email,
            register_token_ttl_secs,
        }
    }
}

impl Default for ViewerAntiAbusePayload {
    fn default() -> Self {
        Self {
            verify_email_rate_limit_window_secs: None,
            verify_email_rate_limit_max: None,
            verify_email_rate_limit_email_max: None,
            verify_email_cooldown_secs: None,
            block_disposable_email: None,
            block_edu_gov_email: None,
            register_token_ttl_secs: None,
        }
    }
}

async fn resolve_viewer_anti_abuse_config(
    pool: &MySqlPool,
    defaults: &ViewerAntiAbuseConfig,
) -> ViewerAntiAbuseConfig {
    let raw = get_json(pool, "viewer_anti_abuse", json!({})).await;
    let payload: ViewerAntiAbusePayload = serde_json::from_value(raw).unwrap_or_default();
    let mut cfg = defaults.clone();
    if let Some(value) = payload.verify_email_rate_limit_window_secs {
        cfg.verify_email_rate_limit_window_secs = value.clamp(60, 86400);
    }
    if let Some(value) = payload.verify_email_rate_limit_max {
        cfg.verify_email_rate_limit_max = value.clamp(1, 20);
    }
    if let Some(value) = payload.verify_email_rate_limit_email_max {
        cfg.verify_email_rate_limit_email_max = value.clamp(1, 20);
    }
    if let Some(value) = payload.verify_email_cooldown_secs {
        cfg.verify_email_cooldown_secs = value.clamp(60, 7200);
    }
    if let Some(value) = payload.block_disposable_email {
        cfg.block_disposable_email = value;
    }
    if let Some(value) = payload.block_edu_gov_email {
        cfg.block_edu_gov_email = value;
    }
    if let Some(value) = payload.register_token_ttl_secs {
        cfg.register_token_ttl_secs = value.clamp(60, 3600);
    }
    cfg
}

fn client_ip(headers: &HeaderMap) -> Option<String> {
    let candidates = [
        "cf-connecting-ip",
        "x-forwarded-for",
        "x-real-ip",
    ];
    for key in candidates {
        if let Some(value) = headers.get(key) {
            if let Ok(raw) = value.to_str() {
                let ip = raw.split(',').next().unwrap_or("").trim();
                if !ip.is_empty() {
                    return Some(ip.to_string());
                }
            }
        }
    }
    None
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

async fn get_stream_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    let status = stream
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("offline");
    let is_live = matches!(status, "live" | "ready");
    Json(json!({ "status": status, "live": is_live }))
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
    let webrtc_base = public_media_base(&state.mediamtx_webrtc, &headers);
    let hls_base = public_media_base(&state.mediamtx_hls, &headers);
    let ll_hls = state.mediamtx_ll_hls.as_ref().map(|base| public_media_base(base, &headers));
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
    let ll_hls_url = ll_hls.map(|base| {
        format!(
            "{}/{}/index.m3u8?token={}",
            base.trim_end_matches('/'),
            state.mediamtx_path,
            read
        )
    });
    let mut modes = vec!["whep"];
    if ll_hls_url.is_some() {
        modes.push("ll-hls");
    }
    modes.push("hls");
    Json(json!({
        "whep": whep,
        "llHls": ll_hls_url,
        "hls": hls,
        "token": read,
        "modes": modes
    }))
}

async fn get_stream_replay(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let read = ensure_read_token(&state).await;
    let hls_base = public_media_base(&state.mediamtx_hls, &headers);
    let hls = format!(
        "{}/{}/index.m3u8?token={}",
        hls_base.trim_end_matches('/'),
        state.mediamtx_path,
        read
    );
    let max_seconds = (state.hls_segment_duration_secs * state.hls_segment_count)
        .min(state.replay_max_seconds);
    Json(json!({
        "enabled": true,
        "maxSeconds": max_seconds,
        "segmentDurationSecs": state.hls_segment_duration_secs,
        "segmentCount": state.hls_segment_count,
        "rewindSteps": [10, 30, 60],
        "hls": hls
    }))
}

fn public_media_base(base: &str, headers: &HeaderMap) -> String {
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let (base_host_port, base_path) = rest.split_once('/').unwrap_or((rest, ""));
    let (base_host, _base_port) = split_host_port(base_host_port);
    if !is_local_host(&base_host) {
        return base.to_string();
    }
    let forwarded_host = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.trim().is_empty());
    let host_header = forwarded_host
        .or_else(|| headers.get("host").and_then(|v| v.to_str().ok()));
    let Some(host_header) = host_header else {
        return base.to_string();
    };
    let (host_only, _host_port) = split_host_port(host_header);
    if is_local_host(&host_only) {
        return base.to_string();
    }
    let forwarded_proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.trim().is_empty());
    let scheme = forwarded_proto.unwrap_or(scheme);
    let rebuilt_host = host_header.to_string();
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

async fn get_live_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    let status = stream
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("offline");
    let live = matches!(status, "live" | "ready");
    let viewer_count = stream
        .get("viewers")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
        .round() as i64;
    let resolution = stream.get("resolution").and_then(|v| v.as_str()).unwrap_or("-");
    let fps = stream
        .get("fps")
        .and_then(|v| v.as_str())
        .and_then(parse_number)
        .map(|v| v.round() as i64)
        .unwrap_or(0);
    let bitrate = stream
        .get("bitrate")
        .and_then(|v| v.as_str())
        .and_then(parse_number)
        .map(|v| (v * 1000.0) as i64)
        .unwrap_or(0);
    let latency = json!({
        "end_to_end": read_latency_ms(&stream, "latencyE2eMs", "latency"),
        "publish": read_latency_ms(&stream, "publishLatencyMs", "pushLatency"),
        "playback": read_latency_ms(&stream, "playbackLatencyMs", "playoutDelay"),
    });
    let started_at = stream.get("startedAt").and_then(|v| v.as_i64()).unwrap_or(0);
    Json(json!({
        "live": live,
        "title": stream.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        "viewer_count": viewer_count,
        "resolution": resolution,
        "fps": fps,
        "bitrate": bitrate,
        "latency": latency,
        "started_at": started_at
    }))
}

async fn get_live_stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    let status = stream
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("offline");
    let live = matches!(status, "live" | "ready");
    let started_at = stream.get("startedAt").and_then(|v| v.as_i64()).unwrap_or(0);
    let latency = json!({
        "end_to_end": read_latency_ms(&stream, "latencyE2eMs", "latency"),
        "publish": read_latency_ms(&stream, "publishLatencyMs", "pushLatency"),
        "playback": read_latency_ms(&stream, "playbackLatencyMs", "playoutDelay"),
    });
    Json(json!({
        "live": live,
        "title": stream.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        "viewer_count": stream.get("viewers").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i64,
        "resolution": stream.get("resolution").and_then(|v| v.as_str()).unwrap_or("-"),
        "fps": stream.get("fps").and_then(|v| v.as_str()).and_then(parse_number).map(|v| v.round() as i64).unwrap_or(0),
        "bitrate": stream.get("bitrate").and_then(|v| v.as_str()).and_then(parse_number).map(|v| (v * 1000.0) as i64).unwrap_or(0),
        "latency": latency,
        "started_at": started_at
    }))
}

async fn get_live_history(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(240).min(1000);
    let since = query.since.unwrap_or(0);
    let rows = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
        r#"
        SELECT ts, viewer_count, bitrate_kbps, fps, latency_e2e_ms, latency_publish_ms, latency_playback_ms
        FROM stream_stats_history
        WHERE ts >= ?
        ORDER BY ts DESC
        LIMIT ?
        "#,
    )
    .bind(since)
    .bind(limit as i64)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let items = rows
        .into_iter()
        .map(|row| {
            json!({
                "ts": row.0,
                "viewer_count": row.1.unwrap_or(0),
                "bitrate": row.2.unwrap_or(0),
                "fps": row.3.unwrap_or(0),
                "latency": {
                    "end_to_end": row.4.unwrap_or(0),
                    "publish": row.5.unwrap_or(0),
                    "playback": row.6.unwrap_or(0),
                }
            })
        })
        .collect::<Vec<_>>();

    Json(json!({ "updatedAt": now_iso(), "items": items }))
}

async fn get_live_countdown(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let schedule = get_json(&state.pool, "schedule", default_kv()[4].1.clone()).await;
    let now = chrono::Utc::now().timestamp();
    let mut next_item: Option<(i64, Value)> = None;
    if let Some(items) = schedule.as_array() {
        for item in items {
            let starts_at = parse_schedule_start(item);
            if let Some(ts) = starts_at {
                if ts > now {
                    if next_item.as_ref().map(|(cur, _)| ts < *cur).unwrap_or(true) {
                        next_item = Some((ts, item.clone()));
                    }
                }
            }
        }
    }
    if let Some((ts, item)) = next_item {
        let remaining = (ts - now).max(0);
        Json(json!({
            "next": {
                "startsAt": ts,
                "title": item.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "host": item.get("host").and_then(|v| v.as_str()).unwrap_or(""),
            },
            "remainingSecs": remaining
        }))
    } else {
        Json(json!({ "next": null, "remainingSecs": 0 }))
    }
}

async fn get_notifications(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let items = get_json(&state.pool, "notifications", json!([])).await;
    Json(json!({ "updatedAt": now_iso(), "items": items }))
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
    let rows = sqlx::query_as::<_, ChatMessageRow>(
        r#"
        SELECT
            cm.id,
            cm.user,
            cm.text,
            DATE_FORMAT(cm.created_at, '%Y-%m-%d %H:%i:%s') as created_at,
            (SELECT COUNT(*) FROM chat_messages cm2 WHERE cm2.user = cm.user AND cm2.id <= cm.id) as msg_count
        FROM chat_messages cm
        ORDER BY cm.id DESC
        LIMIT 50
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let items = rows
        .into_iter()
        .rev()
        .map(|row| {
            let level = chat_level(row.msg_count);
            ChatMessage {
                id: row.id,
                user: row.user,
                text: row.text,
                created_at: row.created_at,
                msg_count: row.msg_count,
                level,
                level_label: format!("Lv{level}"),
            }
        })
        .collect::<Vec<_>>();

    Json(json!({ "items": items }))
}

async fn get_chat_activity(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let count_10s = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM chat_messages WHERE created_at >= (NOW() - INTERVAL 10 SECOND)"
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);
    let heat = chat_heat(count_10s);
    Json(json!({
        "per10s": count_10s,
        "heat": heat
    }))
}

async fn get_chat_leaderboard(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<LeaderboardQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(10).min(100);
    let stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    let started_at = stream.get("startedAt").and_then(|v| v.as_i64()).unwrap_or(0);
    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        r#"
        SELECT user,
               COUNT(*) AS total_count,
               SUM(created_at >= FROM_UNIXTIME(?)) AS live_count
        FROM chat_messages
        GROUP BY user
        ORDER BY total_count DESC
        LIMIT ?
        "#,
    )
    .bind(started_at)
    .bind(limit as i64)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let items = rows
        .into_iter()
        .enumerate()
        .map(|(idx, row)| {
            json!({
                "rank": (idx + 1) as i64,
                "user": row.0,
                "messages": row.1,
                "messages_live": row.2
            })
        })
        .collect::<Vec<_>>();
    Json(json!({ "items": items }))
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
    let mut text = text.chars().take(160).collect::<String>();
    text = replace_chat_emojis(&text);
    let text = text.chars().take(200).collect::<String>();
    if !state.chat_filter_words.is_empty() {
        let lowered = text.to_lowercase();
        if state.chat_filter_words.iter().any(|word| lowered.contains(word)) {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "含有敏感词" })));
        }
    }

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

    let msg_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM chat_messages WHERE user = ?"
    )
    .bind(&user)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);
    let level = chat_level(msg_count);
    let msg = ChatMessage {
        id: result.unwrap().last_insert_id() as i64,
        user,
        text,
        created_at: now_iso(),
        msg_count,
        level,
        level_label: format!("Lv{level}"),
    };

    broadcast(&state, "chat:new", &json!(msg));

    if let Ok(count_10s) = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM chat_messages WHERE created_at >= (NOW() - INTERVAL 10 SECOND)"
    )
    .fetch_one(&state.pool)
    .await
    {
        let heat = chat_heat(count_10s);
        broadcast(&state, "chat:activity", &json!({
            "per10s": count_10s,
            "heat": heat
        }));
    }

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

async fn post_clip_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ClipCreatePayload>,
) -> impl IntoResponse {
    let Some(viewer) = viewer_from_header(&headers, &state).await else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "login_required" })));
    };
    let length_secs = payload.length_secs.unwrap_or(20);
    if length_secs < state.clip_min_secs || length_secs > state.clip_max_secs {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "length_invalid", "min": state.clip_min_secs, "max": state.clip_max_secs })),
        );
    }
    let stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    let status = stream.get("status").and_then(|v| v.as_str()).unwrap_or("offline");
    if status != "live" && status != "ready" {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "stream_offline" })));
    }

    let clip_id = format!("clip_{:x}", rand_u64());
    let clip_url = format!("/clips/{clip_id}.mp4");
    let public_url = public_url(&state, &clip_url);

    let insert = sqlx::query(
        r#"
        INSERT INTO clips (id, created_by, length_secs, status, url)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(&clip_id)
    .bind(&viewer)
    .bind(length_secs as i64)
    .bind("processing")
    .bind(&clip_url)
    .execute(&state.pool)
    .await;

    if insert.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "db" })));
    }

    let task_state = state.clone();
    let clip_id_task = clip_id.clone();
    tokio::spawn(async move {
        info!("clip build started id={} length_secs={}", clip_id_task, length_secs);
        let result = build_clip_from_hls(&task_state, &clip_id_task, length_secs).await;
        if let Err(err) = result {
            info!("clip build failed id={} error={}", clip_id_task, err);
            let _ = sqlx::query("UPDATE clips SET status = ?, error = ? WHERE id = ?")
                .bind("failed")
                .bind(err)
                .bind(&clip_id_task)
                .execute(&task_state.pool)
                .await;
        } else {
            info!("clip build completed id={}", clip_id_task);
            let _ = sqlx::query("UPDATE clips SET status = ? WHERE id = ?")
                .bind("ready")
                .bind(&clip_id_task)
                .execute(&task_state.pool)
                .await;
        }
    });

    (
        StatusCode::OK,
        Json(json!({ "clip_id": clip_id, "clip_url": public_url })),
    )
}

async fn post_stream_telemetry(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<TelemetryPayload>,
) -> impl IntoResponse {
    let mut patch = json!({});
    if let Some(ms) = payload.e2e_latency_ms {
        if let Some(obj) = patch.as_object_mut() {
            obj.insert("latency".to_string(), json!(format!("{:.0}ms", ms)));
            obj.insert("latencyE2eMs".to_string(), json!(ms.round() as i64));
        }
    }
    if let Some(ms) = payload.playout_delay_ms {
        if let Some(obj) = patch.as_object_mut() {
            obj.insert("playoutDelay".to_string(), json!(format!("{:.0}ms", ms)));
            obj.insert("playbackLatencyMs".to_string(), json!(ms.round() as i64));
        }
    }
    if let Some(ms) = payload.push_latency_ms {
        if let Some(obj) = patch.as_object_mut() {
            obj.insert("pushLatency".to_string(), json!(format!("{:.0}ms", ms)));
            obj.insert("publishLatencyMs".to_string(), json!(ms.round() as i64));
        }
    }
    if let Some(fps) = payload.fps {
        if let Some(obj) = patch.as_object_mut() {
            let value = if fps.is_finite() && fps > 0.0 {
                format!("{:.0} fps", fps)
            } else {
                "-".to_string()
            };
            obj.insert("fps".to_string(), json!(value));
            obj.insert("fpsValue".to_string(), json!(fps.round() as i64));
        }
    }
    if patch.as_object().map(|obj| obj.is_empty()).unwrap_or(true) {
        return (StatusCode::OK, Json(json!({ "ok": true })));
    }
    update_stream_patch(&state, &patch).await;
    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn build_clip_from_hls(state: &Arc<AppState>, clip_id: &str, length_secs: u64) -> Result<(), String> {
    let read = ensure_read_token(state).await;
    let playlist_url = format!(
        "{}/{}/index.m3u8?token={}",
        state.mediamtx_hls.trim_end_matches('/'),
        state.mediamtx_path,
        read
    );
    let client = Client::new();
    let playlist = client
        .get(&playlist_url)
        .send()
        .await
        .map_err(|_| "playlist_fetch_failed".to_string())?
        .text()
        .await
        .map_err(|_| "playlist_read_failed".to_string())?;

    let playlist_info = parse_hls_playlist(&playlist, &playlist_url, &read);
    if playlist_info.segments.is_empty() {
        return Err("no_segments".to_string());
    }
    let selected = select_hls_segments(&playlist_info.segments, length_secs as f64);
    if selected.is_empty() {
        return Err("segment_select_failed".to_string());
    }

    let tmp_root = PathBuf::from("resources/clip_tmp").join(clip_id);
    fs::create_dir_all(&tmp_root)
        .await
        .map_err(|_| "tmp_dir_failed".to_string())?;

    let mut list_lines = Vec::new();
    if let Some(init_url) = playlist_info.init_url.as_ref() {
        let init_ext = file_extension_from_url(init_url).unwrap_or("mp4");
        let init_path = tmp_root.join(format!("init.{init_ext}"));
        let init_bytes = client
            .get(init_url)
            .send()
            .await
            .map_err(|_| "init_fetch_failed".to_string())?
            .bytes()
            .await
            .map_err(|_| "init_read_failed".to_string())?;
        fs::write(&init_path, init_bytes)
            .await
            .map_err(|_| "init_write_failed".to_string())?;
        let line = format!("file '{}'", init_path.to_string_lossy().replace('\\', "/"));
        list_lines.push(line);
    }
    for (idx, seg) in selected.iter().enumerate() {
        let ext = file_extension_from_url(&seg.url).unwrap_or("ts");
        let path = tmp_root.join(format!("seg_{idx}.{ext}"));
        let bytes = client
            .get(&seg.url)
            .send()
            .await
            .map_err(|_| "segment_fetch_failed".to_string())?
            .bytes()
            .await
            .map_err(|_| "segment_read_failed".to_string())?;
        fs::write(&path, bytes)
            .await
            .map_err(|_| "segment_write_failed".to_string())?;
        let line = format!("file '{}'", path.to_string_lossy().replace('\\', "/"));
        list_lines.push(line);
    }
    let concat_path = tmp_root.join("concat.txt");
    fs::write(&concat_path, list_lines.join("\n"))
        .await
        .map_err(|_| "concat_write_failed".to_string())?;

    let output_dir = PathBuf::from(&state.clip_dir);
    fs::create_dir_all(&output_dir)
        .await
        .map_err(|_| "clip_dir_failed".to_string())?;
    let output_path = output_dir.join(format!("{clip_id}.mp4"));
    let concat_path_str = concat_path.to_string_lossy().to_string();
    let output_path_str = output_path.to_string_lossy().to_string();
    let args = vec![
        "-y".to_string(),
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-i".to_string(),
        concat_path_str,
        "-c".to_string(),
        "copy".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        output_path_str,
    ];
    run_ffmpeg(&state.ffmpeg_bin, &args).await?;

    let _ = fs::remove_dir_all(&tmp_root).await;
    Ok(())
}

struct HlsSegment {
    url: String,
    duration: f64,
}

struct HlsPlaylist {
    init_url: Option<String>,
    segments: Vec<HlsSegment>,
}

fn parse_hls_playlist(playlist: &str, playlist_url: &str, token: &str) -> HlsPlaylist {
    let base = playlist_base(playlist_url);
    let origin = playlist_origin(playlist_url);
    let mut segments = Vec::new();
    let mut init_url: Option<String> = None;
    let mut current_duration: Option<f64> = None;
    for line in playlist.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#EXT-X-MAP:") {
            if let Some(uri) = parse_hls_attr(trimmed, "URI") {
                let mut url = if uri.starts_with("http://") || uri.starts_with("https://") {
                    uri
                } else if uri.starts_with('/') {
                    format!("{origin}{uri}")
                } else {
                    format!("{base}/{uri}")
                };
                url = append_token(&url, token);
                init_url = Some(url);
            }
            continue;
        }
        if trimmed.starts_with("#EXTINF:") {
            let value = trimmed.trim_start_matches("#EXTINF:");
            let duration = value.split(',').next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            current_duration = Some(duration);
            continue;
        }
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let mut url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else if trimmed.starts_with('/') {
            format!("{origin}{trimmed}")
        } else {
            format!("{base}/{trimmed}")
        };
        url = append_token(&url, token);
        segments.push(HlsSegment {
            url,
            duration: current_duration.unwrap_or(0.0).max(0.1),
        });
        current_duration = None;
    }
    HlsPlaylist { init_url, segments }
}

fn select_hls_segments(segments: &[HlsSegment], target_secs: f64) -> Vec<HlsSegment> {
    let mut acc = 0.0;
    let mut picked = Vec::new();
    for seg in segments.iter().rev() {
        acc += seg.duration;
        picked.push(HlsSegment {
            url: seg.url.clone(),
            duration: seg.duration,
        });
        if acc >= target_secs {
            break;
        }
    }
    picked.reverse();
    picked
}

fn playlist_base(url: &str) -> String {
    let no_query = url.split('?').next().unwrap_or(url);
    if let Some(pos) = no_query.rfind('/') {
        no_query[..pos].to_string()
    } else {
        no_query.to_string()
    }
}

fn playlist_origin(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let host = rest.split('/').next().unwrap_or(rest);
    format!("{scheme}://{host}")
}

fn append_token(url: &str, token: &str) -> String {
    if url.contains("token=") {
        url.to_string()
    } else if url.contains('?') {
        format!("{url}&token={token}")
    } else {
        format!("{url}?token={token}")
    }
}

fn parse_hls_attr(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)?;
    let after = &line[start + key.len()..];
    let after = after.strip_prefix('=')?;
    let after = after.trim_start();
    if after.starts_with('"') {
        let rest = after.trim_start_matches('"');
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }
    let end = after.find(',').unwrap_or(after.len());
    Some(after[..end].to_string())
}

fn file_extension_from_url(url: &str) -> Option<&str> {
    let clean = url.split('?').next().unwrap_or(url);
    std::path::Path::new(clean).extension().and_then(|ext| ext.to_str())
}

async fn run_ffmpeg(bin: &str, args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new(bin);
    for arg in args {
        cmd.arg(arg);
    }
    let status = cmd.status().await.map_err(|_| "ffmpeg_start_failed".to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("ffmpeg_failed".to_string())
    }
}

fn public_url(state: &AppState, path: &str) -> String {
    if state.public_base_url.trim().is_empty() {
        path.to_string()
    } else {
        format!(
            "{}/{}",
            state.public_base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

async fn run_thumbnail_loop(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(state.thumbnail_interval);
    loop {
        ticker.tick().await;
        let stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
        let status = stream.get("status").and_then(|v| v.as_str()).unwrap_or("offline");
        if status != "live" && status != "ready" {
            continue;
        }
        let source = if let Some(src) = state.thumbnail_source.as_ref() {
            src.clone()
        } else {
            let read = ensure_read_token(&state).await;
            format!(
                "{}/{}/index.m3u8?token={}",
                state.mediamtx_hls.trim_end_matches('/'),
                state.mediamtx_path,
                read
            )
        };
        let output_dir = PathBuf::from("public/live");
        if fs::create_dir_all(&output_dir).await.is_err() {
            continue;
        }
        let output_tmp = output_dir.join("cover.tmp.jpg");
        let output_path = output_dir.join("cover.jpg");
        let args = vec![
            "-y".to_string(),
            "-i".to_string(),
            source,
            "-frames:v".to_string(),
            "1".to_string(),
            "-q:v".to_string(),
            "2".to_string(),
            output_tmp.to_string_lossy().to_string(),
        ];
        if run_ffmpeg(&state.ffmpeg_bin, &args).await.is_ok() {
            let _ = fs::rename(&output_tmp, &output_path).await;
        }
    }
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
    headers: HeaderMap,
    Json(payload): Json<LoginPayload>,
) -> impl IntoResponse {
    if cf_access_enabled(&state) && !verify_cf_access(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "access" })));
    }
    if !verify_turnstile(&state, payload.turnstile_token.clone()).await {
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "turnstile" })));
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
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "invalid" })));
    }

    let ttl = ttl_from_remember(state.token_ttl, payload.remember_days);
    let token = generate_key();
    let mut tokens = state.tokens.write().await;
    tokens.insert(token.clone(), Instant::now() + ttl);

    let viewer_token = issue_viewer_token(&state, payload.username.trim().to_string(), true, Some(ttl)).await;
    let mut headers_out = HeaderMap::new();
    append_cookie(&mut headers_out, build_cookie(ADMIN_COOKIE, &token, ttl.as_secs()));
    append_cookie(&mut headers_out, build_cookie(VIEWER_COOKIE, &viewer_token, ttl.as_secs()));
    (
        StatusCode::OK,
        headers_out,
        Json(json!({
            "token": token,
            "expiresIn": ttl.as_secs(),
            "viewerToken": viewer_token,
            "viewerName": payload.username.trim()
        }))
    )
}

async fn admin_access_mode(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "cf_access_enabled": cf_access_enabled(&state)
        })),
    )
}

async fn admin_me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid" })));
    }
    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn admin_logout() -> impl IntoResponse {
    let mut headers_out = HeaderMap::new();
    append_cookie(&mut headers_out, clear_cookie(ADMIN_COOKIE));
    append_cookie(&mut headers_out, clear_cookie(VIEWER_COOKIE));
    (StatusCode::OK, headers_out, Json(json!({ "ok": true })))
}

async fn admin_turnstile_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<AdminTurnstilePayload>,
) -> impl IntoResponse {
    if !cf_access_enabled(&state) {
        return (StatusCode::BAD_REQUEST, HeaderMap::new(), Json(json!({ "error": "access_not_enabled" })));
    }
    if !verify_cf_access(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "access" })));
    }
    if !verify_turnstile(&state, payload.turnstile_token.clone()).await {
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "turnstile" })));
    }

    let ttl = ttl_from_remember(state.token_ttl, payload.remember_days);
    let token = generate_key();
    let mut tokens = state.tokens.write().await;
    tokens.insert(token.clone(), Instant::now() + ttl);

    let viewer_token = issue_viewer_token(&state, "主播".to_string(), true, Some(ttl)).await;
    let mut headers_out = HeaderMap::new();
    append_cookie(&mut headers_out, build_cookie(ADMIN_COOKIE, &token, ttl.as_secs()));
    append_cookie(&mut headers_out, build_cookie(VIEWER_COOKIE, &viewer_token, ttl.as_secs()));
    (
        StatusCode::OK,
        headers_out,
        Json(json!({
            "token": token,
            "expiresIn": ttl.as_secs(),
            "viewerToken": viewer_token,
            "viewerName": "主播"
        }))
    )
}

async fn admin_smtp_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
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

async fn admin_telegram_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let raw = get_json(&state.pool, "telegram_channel", json!("")).await;
    let channel = raw.as_str().unwrap_or("").to_string();
    let token_configured = state
        .telegram_bot_token
        .as_ref()
        .map(|val| !val.trim().is_empty())
        .unwrap_or(false);
    (StatusCode::OK, Json(json!(TelegramChannelPublic { channel, token_configured })))
}

async fn admin_telegram_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<TelegramChannelPayload>,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let channel = payload.channel.trim().to_string();
    set_json(&state.pool, "telegram_channel", &json!(channel)).await;
    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn admin_notify_templates_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let data = get_json(&state.pool, "notify_templates", default_notify_templates()).await;
    (StatusCode::OK, Json(json!({ "data": data })))
}

async fn admin_notify_templates_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    if !payload.is_object() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" })));
    }
    set_json(&state.pool, "notify_templates", &payload).await;
    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn admin_viewer_anti_abuse_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" })));
    }
    let cfg = resolve_viewer_anti_abuse_config(&state.pool, &state.viewer_anti_abuse_defaults).await;
    (
        StatusCode::OK,
        Json(json!(ViewerAntiAbusePublic::from(&cfg))),
    )
}

async fn admin_viewer_anti_abuse_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ViewerAntiAbusePayload>,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" })));
    }
    let mut cfg = resolve_viewer_anti_abuse_config(&state.pool, &state.viewer_anti_abuse_defaults).await;

    if let Some(value) = payload.verify_email_rate_limit_window_secs {
        cfg.verify_email_rate_limit_window_secs = value.clamp(60, 86400);
    }
    if let Some(value) = payload.verify_email_rate_limit_max {
        cfg.verify_email_rate_limit_max = value.clamp(1, 20);
    }
    if let Some(value) = payload.verify_email_rate_limit_email_max {
        cfg.verify_email_rate_limit_email_max = value.clamp(1, 20);
    }
    if let Some(value) = payload.verify_email_cooldown_secs {
        cfg.verify_email_cooldown_secs = value.clamp(60, 7200);
    }
    if let Some(value) = payload.block_disposable_email {
        cfg.block_disposable_email = value;
    }
    if let Some(value) = payload.block_edu_gov_email {
        cfg.block_edu_gov_email = value;
    }
    if let Some(value) = payload.register_token_ttl_secs {
        cfg.register_token_ttl_secs = value.clamp(60, 3600);
    }

    let payload = serde_json::to_value(&cfg).unwrap_or_else(|_| json!({}));
    set_json(&state.pool, "viewer_anti_abuse", &payload).await;

    (
        StatusCode::OK,
        Json(json!(ViewerAntiAbusePublic::from(&cfg))),
    )
}

async fn viewer_register_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<RegisterTokenPayload>,
) -> impl IntoResponse {
    if !verify_turnstile(&state, payload.turnstile_token.clone()).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "turnstile" })));
    }
    let anti_abuse = resolve_viewer_anti_abuse_config(&state.pool, &state.viewer_anti_abuse_defaults).await;
    let now = now_ts();
    let ip = client_ip(&headers).unwrap_or_else(|| "unknown".to_string());
    let recent_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM viewer_register_tokens WHERE ip = ? AND created_at >= ?",
    )
    .bind(&ip)
    .bind(now - anti_abuse.verify_email_rate_limit_window_secs)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);
    if recent_count >= anti_abuse.verify_email_rate_limit_max {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "请求过于频繁，请稍后再试" })),
        );
    }

    let token = generate_key();
    let expires_at = now + anti_abuse.register_token_ttl_secs;
    let _ = sqlx::query(
        "INSERT INTO viewer_register_tokens (token, ip, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&token)
    .bind(&ip)
    .bind(now)
    .bind(expires_at)
    .execute(&state.pool)
    .await;

    (
        StatusCode::OK,
        Json(json!({
            "token": token,
            "expiresIn": anti_abuse.register_token_ttl_secs
        })),
    )
}

async fn viewer_register_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<RegisterStartPayload>,
) -> impl IntoResponse {
    let anti_abuse = resolve_viewer_anti_abuse_config(&state.pool, &state.viewer_anti_abuse_defaults).await;
    let now = now_ts();
    let ip = client_ip(&headers).unwrap_or_else(|| "unknown".to_string());
    let register_token = payload
        .register_token
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    if register_token.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "验证令牌缺失，请刷新后重试" })),
        );
    }
    let reserved = sqlx::query(
        "UPDATE viewer_register_tokens
         SET used_at = ?
         WHERE token = ? AND used_at IS NULL AND expires_at >= ? AND ip = ?",
    )
    .bind(now)
    .bind(&register_token)
    .bind(now)
    .bind(&ip)
    .execute(&state.pool)
    .await
    .map(|res| res.rows_affected())
    .unwrap_or(0);
    if reserved == 0 {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "验证令牌无效，请刷新后重试" })),
        );
    }
    let email = payload.email.trim().to_lowercase();
    if !is_valid_email(&email) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "email" })));
    }
    if anti_abuse.block_disposable_email && is_disposable_email_domain(&email) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "不支持一次性邮箱，请使用常用邮箱" })),
        );
    }
    if anti_abuse.block_edu_gov_email && is_edu_or_gov_email_domain(&email) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "不支持 .edu/.gov 邮箱，请使用常用邮箱" })),
        );
    }

    let recent_ip_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM viewer_email_log WHERE ip = ? AND created_at >= ?",
    )
    .bind(&ip)
    .bind(now - anti_abuse.verify_email_rate_limit_window_secs)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);
    if recent_ip_count >= anti_abuse.verify_email_rate_limit_max {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "发送过于频繁，请稍后再试" })),
        );
    }
    let recent_email_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM viewer_email_log WHERE email = ? AND created_at >= ?",
    )
    .bind(&email)
    .bind(now - anti_abuse.verify_email_rate_limit_window_secs)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);
    if recent_email_count >= anti_abuse.verify_email_rate_limit_email_max {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "该邮箱发送次数已达上限，请稍后再试" })),
        );
    }
    let last_sent: Option<i64> = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(created_at) FROM viewer_email_log WHERE email = ?",
    )
    .bind(&email)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(None);
    if let Some(last_sent) = last_sent {
        if now.saturating_sub(last_sent) < anti_abuse.verify_email_cooldown_secs {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "发送过于频繁，请稍后再试" })),
            );
        }
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
            let _ = sqlx::query(
                "UPDATE viewer_register_tokens SET used_at = NULL WHERE token = ? AND used_at = ?",
            )
            .bind(&register_token)
            .bind(now)
            .execute(&state.pool)
            .await;
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": err })));
        }
    } else if !state.email_echo {
        let _ = sqlx::query(
            "UPDATE viewer_register_tokens SET used_at = NULL WHERE token = ? AND used_at = ?",
        )
        .bind(&register_token)
        .bind(now)
        .execute(&state.pool)
        .await;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "smtp_not_configured" })));
    }

    let _ = sqlx::query(
        "INSERT INTO viewer_email_log (email, ip, created_at) VALUES (?, ?, ?)",
    )
    .bind(&email)
    .bind(&ip)
    .bind(now)
    .execute(&state.pool)
    .await;

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
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "turnstile" })));
    }
    let email = payload.email.trim().to_lowercase();
    let username = payload.username.trim();
    let password = payload.password.trim();
    let code = payload.code.trim();

    if !is_valid_email(&email) || username.is_empty() || password.len() < 6 || code.is_empty() {
        return (StatusCode::BAD_REQUEST, HeaderMap::new(), Json(json!({ "error": "invalid" })));
    }

    let mut pending = state.pending_viewer_codes.write().await;
    let Some(stored) = pending.get(&email) else {
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "code" })));
    };
    if stored.expires_at < Instant::now() || stored.code != code {
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "code" })));
    }

    let mut accounts = get_viewer_accounts(&state.pool).await;
    if accounts.iter().any(|acc| acc.email.eq_ignore_ascii_case(&email)) {
        return (StatusCode::CONFLICT, HeaderMap::new(), Json(json!({ "error": "email_exists" })));
    }
    if accounts.iter().any(|acc| acc.username.eq_ignore_ascii_case(username)) {
        return (StatusCode::CONFLICT, HeaderMap::new(), Json(json!({ "error": "username_exists" })));
    }

    accounts.push(ViewerAccount {
        email: email.clone(),
        username: username.to_string(),
        password_hash: hash_password(password),
        created_at: now_iso(),
    });
    save_viewer_accounts(&state.pool, &accounts).await;
    pending.remove(&email);

    let token = issue_viewer_token(&state, username.to_string(), false, None).await;
    let mut headers_out = HeaderMap::new();
    append_cookie(&mut headers_out, build_cookie(VIEWER_COOKIE, &token, state.viewer_token_ttl.as_secs()));
    (
        StatusCode::OK,
        headers_out,
        Json(json!({ "ok": true, "token": token, "username": username, "expiresIn": state.viewer_token_ttl.as_secs() }))
    )
}

async fn viewer_login(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LoginPayload>,
) -> impl IntoResponse {
    if !verify_turnstile(&state, payload.turnstile_token.clone()).await {
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "turnstile" })));
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
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Json(json!({ "error": "invalid" })));
    }

    let ttl = ttl_from_remember(state.viewer_token_ttl, payload.remember_days);
    let token = issue_viewer_token(&state, username.to_string(), false, Some(ttl)).await;
    let mut headers_out = HeaderMap::new();
    append_cookie(&mut headers_out, build_cookie(VIEWER_COOKIE, &token, ttl.as_secs()));
    (
        StatusCode::OK,
        headers_out,
        Json(json!({ "token": token, "username": username, "expiresIn": ttl.as_secs() }))
    )
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

async fn viewer_logout() -> impl IntoResponse {
    let mut headers_out = HeaderMap::new();
    append_cookie(&mut headers_out, clear_cookie(VIEWER_COOKIE));
    (StatusCode::OK, headers_out, Json(json!({ "ok": true })))
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
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }
    let items = unwrap_items(&payload);
    set_json(&state.pool, "schedule", &items).await;
    broadcast(&state, "schedule:update", &items);
    push_notification(&state, "schedule", "排期已更新", "今日排期已同步").await;
    (StatusCode::OK, Json(json!({ "ok": true, "items": items })))
}

async fn admin_channels_replace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
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
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }

    let room_id = format!("{}", chrono::Utc::now().timestamp());
    let key = ensure_stream_tokens(&state).await;

    let mut stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    if let Some(obj) = stream.as_object_mut() {
        obj.insert("roomId".to_string(), json!(room_id));
        obj.insert("status".to_string(), json!("ready"));
        obj.insert("updatedAt".to_string(), json!(now_iso()));
    }
    set_json(&state.pool, "stream", &stream).await;
    broadcast(&state, "stream:update", &stream);

    let srt_url = format!(
        "srt://127.0.0.1:8890?streamid=publish:{}:token:{}",
        state.mediamtx_path,
        key
    );

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

    let read = ensure_read_token(&state).await;
    (StatusCode::OK, Json(json!({ "ok": true, "roomId": room_id, "key": key, "srtUrl": srt_url, "readToken": read })))
}

async fn admin_ingest_refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_admin_auth(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Invalid API key" })));
    }

    let next = generate_key();
    {
        let mut publish = state.publish_token.write().await;
        *publish = next.clone();
    }
    set_json(&state.pool, "publish_token", &json!(next)).await;

    let srt_url = format!(
        "srt://127.0.0.1:8890?streamid=publish:{}:token:{}",
        state.mediamtx_path,
        next
    );

    (StatusCode::OK, Json(json!({ "ok": true, "srt": srt_url, "token": next })))
}

async fn ws_handler(
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.broadcaster.subscribe();
    let chat_rows = sqlx::query_as::<_, ChatMessageRow>(
        r#"
        SELECT
            cm.id,
            cm.user,
            cm.text,
            DATE_FORMAT(cm.created_at, '%Y-%m-%d %H:%i:%s') as created_at,
            (SELECT COUNT(*) FROM chat_messages cm2 WHERE cm2.user = cm.user AND cm2.id <= cm.id) as msg_count
        FROM chat_messages cm
        ORDER BY cm.id DESC
        LIMIT 50
        "#
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    let chat_items = chat_rows
        .into_iter()
        .rev()
        .map(|row| {
            let level = chat_level(row.msg_count);
            ChatMessage {
                id: row.id,
                user: row.user,
                text: row.text,
                created_at: row.created_at,
                msg_count: row.msg_count,
                level,
                level_label: format!("Lv{level}"),
            }
        })
        .collect::<Vec<_>>();
    let snapshot = json!({
        "stream": get_json(&state.pool, "stream", default_kv()[0].1.clone()).await,
        "stats": get_json(&state.pool, "stats", default_kv()[1].1.clone()).await,
        "health": get_json(&state.pool, "health", default_kv()[2].1.clone()).await,
        "ingestConfig": get_json(&state.pool, "ingest_config", default_kv()[3].1.clone()).await,
        "schedule": get_json(&state.pool, "schedule", default_kv()[4].1.clone()).await,
        "channels": get_json(&state.pool, "channels", default_kv()[5].1.clone()).await,
        "opsAlerts": get_json(&state.pool, "ops_alerts", default_kv()[6].1.clone()).await,
        "nodes": get_json(&state.pool, "nodes", default_kv()[7].1.clone()).await,
        "notifications": get_json(&state.pool, "notifications", json!([])).await,
        "chat": {
            "items": chat_items
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
    let Some(token) = token_from_headers(headers, ADMIN_COOKIE) else { return false };

    let mut tokens = state.tokens.write().await;
    let now = Instant::now();
    tokens.retain(|_, exp| *exp > now);
    tokens.get(&token).is_some()
}

async fn check_admin_auth(headers: &HeaderMap, state: &AppState) -> bool {
    if !check_bearer(headers, state).await {
        return false;
    }
    if !cf_access_enabled(state) {
        return true;
    }
    verify_cf_access(headers, state).await
}

fn cf_access_enabled(state: &AppState) -> bool {
    state
        .cf_access_team_domain
        .as_ref()
        .is_some_and(|v| !v.trim().is_empty())
        && state
            .cf_access_aud
            .as_ref()
            .is_some_and(|v| !v.trim().is_empty())
}

async fn verify_cf_access(headers: &HeaderMap, state: &AppState) -> bool {
    let Some(token) = headers
        .get("CF-Access-Jwt-Assertion")
        .and_then(|val| val.to_str().ok())
    else {
        return false;
    };
    verify_cf_access_token(state, token).await
}

async fn verify_cf_access_token(state: &AppState, token: &str) -> bool {
    let Some(aud) = state.cf_access_aud.as_ref() else {
        return false;
    };
    let header = match decode_header(token) {
        Ok(h) => h,
        Err(_) => return false,
    };
    let kid = header.kid.unwrap_or_default();
    let Some(keys) = load_cf_access_jwks(state).await else {
        return false;
    };
    let key = keys
        .iter()
        .find(|item| !kid.is_empty() && item.kid == kid)
        .or_else(|| keys.first());
    let Some(key) = key else { return false };
    let decoding_key = match DecodingKey::from_rsa_components(&key.n, &key.e) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[aud.as_str()]);
    validation.validate_exp = true;
    validation.leeway = 60;
    decode::<CfAccessClaims>(token, &decoding_key, &validation).is_ok()
}

async fn load_cf_access_jwks(state: &AppState) -> Option<Vec<CfAccessJwk>> {
    let team = state.cf_access_team_domain.as_ref()?;
    {
        let cache = state.cf_access_jwks.read().await;
        if let Some(cache) = cache.as_ref() {
            if cache.fetched_at.elapsed() < Duration::from_secs(600) {
                return Some(cache.keys.clone());
            }
        }
    }

    let host = if team.contains('.') {
        team.trim().to_string()
    } else {
        format!("{}.cloudflareaccess.com", team.trim())
    };
    let url = format!("https://{}/cdn-cgi/access/certs", host);
    let resp = Client::new().get(&url).send().await.ok()?;
    let payload = resp.json::<CfAccessJwks>().await.ok()?;
    let mut cache = state.cf_access_jwks.write().await;
    *cache = Some(CfAccessJwksCache {
        fetched_at: Instant::now(),
        keys: payload.keys.clone(),
    });
    Some(payload.keys)
}

async fn viewer_from_header(headers: &HeaderMap, state: &AppState) -> Option<String> {
    let Some(token) = token_from_headers(headers, VIEWER_COOKIE) else { return None };

    let mut tokens = state.viewer_tokens.write().await;
    let now = Instant::now();
    tokens.retain(|_, session| session.expires_at > now);
    tokens.get(&token).map(|session| {
        if session.is_host {
            format!("主播·{}", session.username)
        } else {
            session.username.clone()
        }
    })
}

async fn viewer_session_from_header(headers: &HeaderMap, state: &AppState) -> Option<(String, String)> {
    let Some(token) = token_from_headers(headers, VIEWER_COOKIE) else { return None };

    let mut tokens = state.viewer_tokens.write().await;
    let now = Instant::now();
    tokens.retain(|_, session| session.expires_at > now);
    tokens.get(&token).map(|session| (token.clone(), session.username.clone()))
}

async fn issue_viewer_token(
    state: &AppState,
    username: String,
    is_host: bool,
    ttl_override: Option<Duration>,
) -> String {
    let token = generate_key();
    let mut tokens = state.viewer_tokens.write().await;
    let ttl = ttl_override.unwrap_or(state.viewer_token_ttl);
    tokens.insert(
        token.clone(),
        ViewerSession {
            username,
            expires_at: Instant::now() + ttl,
            is_host,
        },
    );
    token
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
    let value = email.trim();
    let mut parts = value.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return false;
    }
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

fn normalize_host(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('.')
        .trim_start_matches("www.")
        .to_lowercase()
}

fn is_disposable_email_domain(email: &str) -> bool {
    let domain = email
        .rsplit('@')
        .next()
        .map(normalize_host)
        .unwrap_or_default();
    if domain.is_empty() {
        return true;
    }
    let suffix_rules = [
        "mailinator.com",
        "guerrillamail.com",
        "yopmail.com",
        "tempmail",
        "10minutemail",
        "dropmail",
        "sharklasers.com",
        "dispostable.com",
        "maildrop.cc",
    ];
    suffix_rules
        .iter()
        .any(|rule| domain == *rule || domain.contains(rule))
}

fn is_edu_or_gov_email_domain(email: &str) -> bool {
    let domain = email
        .rsplit('@')
        .next()
        .map(normalize_host)
        .unwrap_or_default();
    if domain.is_empty() {
        return false;
    }
    domain == "edu"
        || domain.ends_with(".edu")
        || domain.ends_with(".edu.cn")
        || domain == "gov"
        || domain.ends_with(".gov")
        || domain.ends_with(".gov.cn")
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
    fn escape_html(input: &str) -> String {
        input
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\n', "<br>")
    }

    let body_text = body_tpl.replace("{code}", code);
    let body_html = {
        let safe_subject = escape_html(subject);
        let safe_body = escape_html(&body_text);
        let safe_code = escape_html(code);
        let code_block = if !safe_code.trim().is_empty() {
            format!(
                r#"<div style="margin:16px 0;display:inline-block;background:#f7f1ff;border:1px solid #e6dcff;color:#5b3fa7;padding:10px 14px;border-radius:999px;font-size:16px;font-weight:700;letter-spacing:0.06em;">{}</div>"#,
                safe_code
            )
        } else {
            String::new()
        };

        format!(
            r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width,initial-scale=1" />
  <title>{subject}</title>
</head>
<body style="margin:0;padding:0;background:#f7f5ff;font-family:'Segoe UI','PingFang SC','Hiragino Sans GB','Microsoft YaHei',sans-serif;color:#2b2440;">
  <div style="max-width:560px;margin:0 auto;padding:32px 18px;">
    <div style="background:linear-gradient(135deg,#fdf3f8,#f1f3ff);border:1px solid #eadff7;border-radius:18px;padding:28px 26px;box-shadow:0 12px 24px rgba(83,63,145,0.08);">
      <div style="font-size:12px;letter-spacing:0.22em;text-transform:uppercase;color:#9a8cc4;">Meow Live Room</div>
      <h1 style="margin:10px 0 6px;font-size:20px;color:#2c204f;">{subject}</h1>
      <div style="height:1px;background:rgba(140,120,190,0.18);margin:12px 0 16px;"></div>
      <div style="font-size:14px;line-height:1.7;color:#3a2f56;">{body}</div>
      {code_block}
      <div style="margin-top:18px;font-size:12px;color:#9a8cc4;">如果不是你本人操作，请忽略这封邮件。</div>
    </div>
    <div style="margin-top:16px;text-align:center;font-size:12px;color:#b7a9d9;">
      © 2026 Meowhuan Live Room
    </div>
  </div>
</body>
</html>"#,
            subject = safe_subject,
            body = safe_body,
            code_block = code_block
        )
    };

    let mut builder = MailMessage::builder()
        .from(config.from.parse().map_err(|_| "smtp_from_invalid")?)
        .to(to.parse().map_err(|_| "smtp_to_invalid")?)
        .subject(subject);
    if let Some(reply_to) = &config.reply_to {
        builder = builder.reply_to(reply_to.parse().map_err(|_| "smtp_reply_invalid")?);
    }
    let email = builder
        .header(ContentType::TEXT_HTML)
        .body(body_html)
        .map_err(|_| "smtp_build_failed")?;

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
    let previous_status = current
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("offline")
        .to_string();
    merge_object(&mut current, patch);
    if let Some(obj) = current.as_object_mut() {
        obj.insert("updatedAt".to_string(), json!(now_iso()));
    }
    let next_status = current
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("offline")
        .to_string();
    if previous_status != next_status {
        if next_status == "live" {
            if let Some(obj) = current.as_object_mut() {
                obj.insert("startedAt".to_string(), json!(now_ts()));
            }
            push_notification(state, "live", "主播已开播", "直播间正在进行中").await;
        } else if previous_status == "live" {
            if let Some(obj) = current.as_object_mut() {
                obj.insert("endedAt".to_string(), json!(now_ts()));
            }
            push_notification(state, "offline", "主播已下播", "直播已结束").await;
        }
    }
    record_stream_stats_if_due(state, &current).await;
    set_json(&state.pool, "stream", &current).await;
    broadcast(state, "stream:update", &current);
}

async fn record_stream_stats_if_due(state: &AppState, stream: &Value) {
    let status = stream.get("status").and_then(|v| v.as_str()).unwrap_or("offline");
    if status != "live" && status != "ready" {
        return;
    }
    let mut last = state.stats_last_written.write().await;
    if last.elapsed() < Duration::from_secs(5) {
        return;
    }
    *last = Instant::now();

    let viewer_count = stream
        .get("viewers")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
        .round() as i64;
    let bitrate_kbps = stream
        .get("bitrate")
        .and_then(|v| v.as_str())
        .and_then(parse_number)
        .map(|v| (v * 1000.0) as i64)
        .unwrap_or(0);
    let fps = stream
        .get("fpsValue")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            stream.get("fps")
                .and_then(|v| v.as_str())
                .and_then(parse_number)
                .map(|v| v.round() as i64)
        })
        .unwrap_or(0);
    let latency_e2e = read_latency_ms(stream, "latencyE2eMs", "latency");
    let latency_publish = read_latency_ms(stream, "publishLatencyMs", "pushLatency");
    let latency_playback = read_latency_ms(stream, "playbackLatencyMs", "playoutDelay");
    let ts = now_ts();

    let _ = sqlx::query(
        r#"
        INSERT INTO stream_stats_history
            (ts, viewer_count, bitrate_kbps, fps, latency_e2e_ms, latency_publish_ms, latency_playback_ms)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(ts)
    .bind(viewer_count)
    .bind(bitrate_kbps)
    .bind(fps)
    .bind(latency_e2e)
    .bind(latency_publish)
    .bind(latency_playback)
    .execute(&state.pool)
    .await;
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

fn get_template_value<'a>(templates: &'a Value, kind: &str) -> Option<&'a Value> {
    templates.get(kind).and_then(|val| val.as_object()).map(|_| templates.get(kind).unwrap())
}

fn render_template(template: &str, ctx: &HashMap<String, String>, captures: Option<&regex::Captures>) -> String {
    let mut out = template.to_string();
    for (key, value) in ctx {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    if let Some(caps) = captures {
        for idx in 1..caps.len() {
            if let Some(value) = caps.get(idx).map(|m| m.as_str()) {
                out = out.replace(&format!("${idx}"), value);
            }
        }
    }
    out
}

fn resolve_notify_template(
    templates: &Value,
    kind: &str,
    ctx: &HashMap<String, String>,
    fallback_title: &str,
    fallback_message: &str,
    fallback_url: &str,
) -> (String, String, String) {
    if let Some(rules) = templates.get("rules").and_then(|v| v.as_array()) {
        for rule in rules {
            let rule_kind = rule.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            if rule_kind != kind {
                continue;
            }
            let field = rule.get("field").and_then(|v| v.as_str()).unwrap_or("title");
            let pattern = rule.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let target = ctx.get(field).cloned().unwrap_or_default();
            if pattern.is_empty() {
                continue;
            }
            if let Ok(re) = regex::Regex::new(pattern) {
                if let Some(caps) = re.captures(&target) {
                    let title_tpl = rule.get("title").and_then(|v| v.as_str()).unwrap_or(fallback_title);
                    let msg_tpl = rule.get("message").and_then(|v| v.as_str()).unwrap_or(fallback_message);
                    let url_tpl = rule.get("url").and_then(|v| v.as_str()).unwrap_or(fallback_url);
                    return (
                        render_template(title_tpl, ctx, Some(&caps)),
                        render_template(msg_tpl, ctx, Some(&caps)),
                        render_template(url_tpl, ctx, Some(&caps)),
                    );
                }
            }
        }
    }
    let tpl = get_template_value(templates, kind);
    let title_tpl = tpl.and_then(|v| v.get("title")).and_then(|v| v.as_str()).unwrap_or(fallback_title);
    let msg_tpl = tpl.and_then(|v| v.get("message")).and_then(|v| v.as_str()).unwrap_or(fallback_message);
    let url_tpl = tpl.and_then(|v| v.get("url")).and_then(|v| v.as_str()).unwrap_or(fallback_url);
    (
        render_template(title_tpl, ctx, None),
        render_template(msg_tpl, ctx, None),
        render_template(url_tpl, ctx, None),
    )
}

fn normalize_telegram_chat_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('@') || trimmed.starts_with("-100") {
        return Some(trimmed.to_string());
    }
    if trimmed.chars().all(|c| c == '-' || c.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }
    let normalized = trimmed
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    let normalized = normalized
        .strip_prefix("t.me/")
        .or_else(|| normalized.strip_prefix("telegram.me/"))
        .unwrap_or(normalized);
    let segment = normalized.split('?').next().unwrap_or("");
    let segment = segment.split('/').next().unwrap_or("");
    if segment.is_empty() || segment.starts_with('+') || segment.contains("joinchat") {
        return None;
    }
    Some(format!("@{segment}"))
}

fn token_from_headers(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    if let Some(token) = headers
        .get("authorization")
        .and_then(|val| val.to_str().ok())
        .and_then(|val| val.strip_prefix("Bearer "))
        .map(|val| val.to_string())
    {
        return Some(token);
    }
    let cookie_header = headers.get("cookie").and_then(|val| val.to_str().ok())?;
    for part in cookie_header.split(';') {
        let trimmed = part.trim();
        let Some((name, value)) = trimmed.split_once('=') else { continue };
        if name == cookie_name {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn build_cookie(name: &str, value: &str, max_age_secs: u64) -> String {
    format!(
        "{name}={value}; Path=/; Max-Age={max_age_secs}; HttpOnly; SameSite=Lax"
    )
}

fn clear_cookie(name: &str) -> String {
    format!("{name}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax")
}

fn append_cookie(headers: &mut HeaderMap, cookie: String) {
    if let Ok(value) = cookie.parse() {
        headers.append(SET_COOKIE, value);
    }
}

async fn send_telegram_message(token: &str, chat_id: &str, text: &str) -> Result<(), String> {
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let client = Client::new();
    let res = client
        .post(url)
        .json(&json!({
            "chat_id": chat_id,
            "text": text,
            "disable_web_page_preview": true
        }))
        .send()
        .await
        .map_err(|_| "telegram_send_failed")?;
    if !res.status().is_success() {
        return Err("telegram_send_failed".to_string());
    }
    Ok(())
}

#[derive(Deserialize)]
struct ViewerSubscriptionPayload {
    live: Option<bool>,
    schedule: Option<bool>,
    email: Option<bool>,
}

async fn push_notification(state: &AppState, kind: &str, title: &str, message: &str) {
    let mut items = get_json(&state.pool, "notifications", json!([])).await;
    if !items.is_array() {
        items = json!([]);
    }
    let list = items.as_array_mut().unwrap();
    let templates = get_json(&state.pool, "notify_templates", default_notify_templates()).await;
    let stream = get_json(&state.pool, "stream", default_kv()[0].1.clone()).await;
    let schedule = get_json(&state.pool, "schedule", default_kv()[4].1.clone()).await;
    let live_url = templates
        .get("live_url")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| state.public_base_url.clone());
    let stream_title = stream.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let stream_host = stream.get("host").and_then(|v| v.as_str()).unwrap_or("");
    let stream_room = stream.get("roomId").and_then(|v| v.as_str()).unwrap_or("");
    let stream_status = stream.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let (schedule_time, schedule_title, schedule_host) = schedule
        .as_array()
        .and_then(|arr| arr.first())
        .map(|first| {
            (
                first.get("time").and_then(|v| v.as_str()).unwrap_or(""),
                first.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                first.get("host").and_then(|v| v.as_str()).unwrap_or(""),
            )
        })
        .unwrap_or(("", "", ""));
    let mut ctx = HashMap::new();
    ctx.insert("title".to_string(), stream_title.to_string());
    ctx.insert("host".to_string(), stream_host.to_string());
    ctx.insert("roomId".to_string(), stream_room.to_string());
    ctx.insert("status".to_string(), stream_status.to_string());
    ctx.insert("time".to_string(), now_iso());
    ctx.insert("liveUrl".to_string(), live_url.clone());
    ctx.insert("scheduleTime".to_string(), schedule_time.to_string());
    ctx.insert("scheduleTitle".to_string(), schedule_title.to_string());
    ctx.insert("scheduleHost".to_string(), schedule_host.to_string());
    let (title, message, url) = resolve_notify_template(
        &templates,
        kind,
        &ctx,
        title,
        message,
        "",
    );
    let mut final_message = message;
    let final_url = if !url.is_empty() { url } else if kind == "live" { live_url } else { String::new() };
    if !final_url.is_empty() && !final_message.contains(&final_url) && !final_message.contains("{liveUrl}") {
        final_message = format!("{final_message}\n\n观看地址：{final_url}");
    }
    list.push(json!({
        "id": now_ts(),
        "type": kind,
        "title": title,
        "message": final_message,
        "url": final_url,
        "createdAt": now_iso()
    }));
    if list.len() > 50 {
        let overflow = list.len() - 50;
        list.drain(0..overflow);
    }
    let last = list.last().cloned();
    set_json(&state.pool, "notifications", &items).await;
    if let Some(last) = last {
        broadcast(state, "notify:new", &last);
    }

    let kind = kind.to_string();
    let title = title.to_string();
    let message = final_message.to_string();
    let url = final_url.to_string();
    let pool = state.pool.clone();
    let smtp = state.smtp.read().await.clone();
    let telegram_bot_token = state.telegram_bot_token.clone();

    tokio::spawn(async move {
        let subs_map = get_json(&pool, "viewer_subscriptions", json!({})).await;
        let mut recipients = Vec::new();
        if let Some(map) = subs_map.as_object() {
            for (username, cfg) in map {
                let live = cfg.get("live").and_then(|v| v.as_bool()).unwrap_or(false);
                let schedule = cfg.get("schedule").and_then(|v| v.as_bool()).unwrap_or(false);
                let email = cfg.get("email").and_then(|v| v.as_bool()).unwrap_or(false);
                let ok = (kind == "live" || kind == "offline") && live || kind == "schedule" && schedule;
                if ok {
                    recipients.push((username.clone(), email));
                }
            }
        }

        if recipients.is_empty() {
            // no-op
        }

        let accounts = get_viewer_accounts(&pool).await;
        let mut email_targets = Vec::new();
        for (username, want_email) in recipients.iter() {
            if !*want_email {
                continue;
            }
            if let Some(acc) = accounts.iter().find(|acc| acc.username.eq_ignore_ascii_case(username)) {
                email_targets.push(acc.email.clone());
            }
        }

        if let (Some(smtp_cfg), false) = (smtp.clone(), email_targets.is_empty()) {
            let body = format!("{message}\n\n时间：{time}", time = now_iso());
            for to in email_targets {
                let _ = send_email_code_with_subject(&smtp_cfg, &to, "", &title, &body).await;
            }
        }

        if let Some(token) = telegram_bot_token.as_ref() {
            let channel_raw = get_json(&pool, "telegram_channel", json!("")).await;
            let channel = channel_raw.as_str().unwrap_or("");
            if let Some(chat_id) = normalize_telegram_chat_id(channel) {
                let mut text = format!("{title}\n{message}\n\n时间：{time}", time = now_iso());
                if !url.is_empty() && !text.contains(&url) {
                    text = format!("{text}\n{url}");
                }
                let _ = send_telegram_message(token, &chat_id, &text).await;
            }
        }
    });
}

async fn viewer_subscription_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let session = viewer_session_from_header(&headers, &state).await;
    let Some((_token, username)) = session else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid" })));
    };
    let map = get_json(&state.pool, "viewer_subscriptions", json!({})).await;
    let entry = map
        .get(&username)
        .cloned()
        .unwrap_or_else(|| json!({ "live": false, "schedule": false, "email": false }));
    (StatusCode::OK, Json(json!({
        "live": entry.get("live").and_then(|v| v.as_bool()).unwrap_or(false),
        "schedule": entry.get("schedule").and_then(|v| v.as_bool()).unwrap_or(false),
        "email": entry.get("email").and_then(|v| v.as_bool()).unwrap_or(false)
    })))
}

async fn viewer_subscription_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ViewerSubscriptionPayload>,
) -> impl IntoResponse {
    let session = viewer_session_from_header(&headers, &state).await;
    let Some((_token, username)) = session else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid" })));
    };
    let mut map = get_json(&state.pool, "viewer_subscriptions", json!({})).await;
    if !map.is_object() {
        map = json!({});
    }
    let live = payload.live.unwrap_or(false);
    let schedule = payload.schedule.unwrap_or(false);
    let email = payload.email.unwrap_or(false);
    if let Some(obj) = map.as_object_mut() {
        obj.insert(username.clone(), json!({ "live": live, "schedule": schedule, "email": email }));
    }
    set_json(&state.pool, "viewer_subscriptions", &map).await;
    (StatusCode::OK, Json(json!({ "ok": true, "live": live, "schedule": schedule, "email": email })))
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
    let patch = json!({ "status": status });
    update_stream_patch(state, &patch).await;
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
            if let Some(ms) = parse_duration_ms(&push_latency) {
                obj.insert("publishLatencyMs".to_string(), json!(ms));
            }
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
    set_json(&state.pool, "publish_token", &json!(token)).await;
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
    set_json(&state.pool, "read_token", &json!(token)).await;
    token
}
