<script setup>
import { ref, computed, watch, nextTick, onMounted, onBeforeUnmount } from "vue";

const isNight = ref(false);
const loading = ref(true);
const error = ref("");
const wsStatus = ref("disconnected");

const stream = ref({
  roomId: "-",
  title: "-",
  host: "-",
  resolution: "-",
  status: "offline",
  bitrate: "-",
  latency: "-",
  pushLatency: "-",
  playoutDelay: "-",
  fps: "-",
  chatRate: "-",
  viewers: 0,
  updatedAt: ""
});

const scheduleList = ref([]);
const opsAlerts = ref([]);
const notifications = ref([]);
const viewerSubscriptions = ref({ live: false, schedule: false, email: false });
const notifyBanner = ref("");

const chatMessages = ref([]);
const loadStoredToken = (tokenKey, expiresKey, identityKey) => {
  const token = localStorage.getItem(tokenKey) || "";
  const expiresAt = Number(localStorage.getItem(expiresKey) || 0);
  if (token && expiresAt && Date.now() > expiresAt) {
    localStorage.removeItem(tokenKey);
    localStorage.removeItem(expiresKey);
    if (identityKey) localStorage.removeItem(identityKey);
    return "";
  }
  return token;
};

const viewerToken = ref(loadStoredToken("viewer_token", "viewer_token_expires_at", "viewer_identity"));
const viewerIdentity = ref(localStorage.getItem("viewer_identity") || "");
const viewerSessionOk = ref(false);
const viewerAuthed = computed(() => !!viewerToken.value || viewerSessionOk.value);
const viewerMode = ref("login");
const viewerEmail = ref("");
const viewerUsername = ref(localStorage.getItem("viewer_user") || "");
const viewerPassword = ref(localStorage.getItem("viewer_pass") || "");
const viewerRememberUser = ref(!!localStorage.getItem("viewer_user"));
const viewerRememberPass = ref(!!localStorage.getItem("viewer_pass"));
const viewerRememberDays = ref(Number(localStorage.getItem("viewer_remember_days") || 0) || 0);
const viewerCode = ref("");
const viewerNotice = ref("");
const viewerSending = ref(false);
const viewerTurnstileToken = ref("");
const viewerRegisterToken = ref("");
const viewerRegisterTokenExpiresAt = ref(0);
const showViewerAuth = ref(false);
const showViewerProfile = ref(false);
const viewerProfile = ref({ username: "", email: "" });
const viewerProfileSaving = ref(false);
const turnstileLoginRef = ref(null);
const turnstileRegisterRef = ref(null);
let turnstileLoginId = null;
let turnstileRegisterId = null;
let turnstileScriptLoading = false;

const chatUser = ref(viewerIdentity.value || localStorage.getItem("viewer_name") || "观众");
const chatText = ref("");
const chatSending = ref(false);
const displayedMessages = computed(() => chatMessages.value);
const chatRateLocal = computed(() => {
  const now = Date.now();
  const count = chatMessages.value.filter((msg) => now - (msg.ts || 0) <= 60000).length;
  return `${count} / min`;
});

const playInfo = ref({ whep: "", llHls: "", hls: "", modes: [] });
const playerError = ref("");
const streamUnavailable = computed(() => {
  const message = playerError.value || "";
  return message.includes("no stream is available on path") || message.includes("未开播");
});
const playerStatus = ref("idle");
const videoRef = ref(null);
const videoFrameRef = ref(null);
let fpsFrameCount = 0;
let fpsWindowStart = 0;
let fpsHandle = 0;
const chatListRef = ref(null);
const chatScrollRef = ref(null);
let pc = null;
let statsTimer = null;
let resizeObserver = null;
let collapseMedia = null;
let collapseListener = null;
let leaderboardTimer = null;
let activityTimer = null;
let drawerRaf = 0;
const drawerStyle = ref({ top: "0px", right: "0px" });
const overlayTab = ref("schedule");
const isCollapsed = ref(false);
const chatAtBottom = ref(true);
const chatHasNew = ref(false);
const isMobile = ref(false);
const selectedMode = ref(localStorage.getItem("playback_mode") || "auto");
const activeMode = ref("auto");
const replayInfo = ref({ enabled: false, maxSeconds: 0, rewindSteps: [10, 30, 60], hls: "" });
const clipCreating = ref(false);
const clipNotice = ref("");
const lastClipUrl = ref("");
const chatActivity = ref({ per10s: 0, heat: "Low" });
const leaderboard = ref([]);
const countdown = ref({ next: null, remainingSecs: 0 });
let countdownTimer = null;
const showMobileControls = ref(false);
const replayExpanded = ref(false);
const showMobileDrawer = ref(false);
const showUnmutePrompt = ref(false);
const unmuteTried = ref(false);
const replayNotice = ref("");
const replayWasWhep = ref(false);

const normalizePlayUrl = (raw) => {
  if (!raw) return raw;
  try {
    const url = new URL(raw, window.location.origin);
    const host = url.hostname;
    if (["127.0.0.1", "localhost", "::1"].includes(host)) {
      url.hostname = window.location.hostname;
    }
    return url.toString();
  } catch {
    return raw;
  }
};

const scrollChatToBottom = () => {
  const container = chatScrollRef.value;
  if (!container) return;
  container.scrollTop = container.scrollHeight;
  chatHasNew.value = false;
  chatAtBottom.value = true;
};

const onChatScroll = () => {
  const container = chatScrollRef.value;
  if (!container) return;
  const threshold = 24;
  const atBottom = container.scrollHeight - container.scrollTop - container.clientHeight <= threshold;
  chatAtBottom.value = atBottom;
  if (atBottom) {
    chatHasNew.value = false;
  }
};

const addMention = (name) => {
  const mention = `@${name} `;
  if (!chatText.value.includes(mention)) {
    chatText.value = `${mention}${chatText.value}`.trimStart();
  }
};

const highlightMentions = (text) => {
  const escaped = text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
  return escaped.replace(/@([^\s@]{1,20})/g, "<span class=\"chat-mention\">@$1</span>");
};

const appendEmoji = (code) => {
  if (!viewerAuthed.value) {
    showViewerAuth.value = true;
    return;
  }
  chatText.value = `${chatText.value}${chatText.value ? " " : ""}${code} `;
  nextTick(() => {});
};

const resolveLevelLabel = (label) => {
  if (!label) return "";
  const raw = String(label).trim();
  const match = raw.match(/(\d+)/);
  if (!match) return raw;
  const level = Number(match[1]);
  if (!Number.isFinite(level) || level <= 0) return "";
  return `Lv${level}`;
};

const resolveLevelClass = (label) => {
  const raw = String(label || "");
  const match = raw.match(/(\d+)/);
  if (!match) return "";
  const level = Number(match[1]);
  if (!Number.isFinite(level) || level <= 0) return "";
  if (level >= 5) return "chat-level-5";
  return `chat-level-${level}`;
};


const reportWhepStats = async () => {
  if (!pc || pc.connectionState === "closed") return;
  try {
    const stats = await pc.getStats();
    let rttMs = null;
    let playoutDelayMs = null;
    let fps = null;
    stats.forEach((stat) => {
      if (stat.type === "candidate-pair" && stat.state === "succeeded" && stat.currentRoundTripTime) {
        rttMs = stat.currentRoundTripTime * 1000;
      }
      if (stat.type === "inbound-rtp" && stat.kind === "video") {
        if (stat.jitterBufferEmittedCount) {
          const delay = (stat.jitterBufferDelay / stat.jitterBufferEmittedCount) * 1000;
          playoutDelayMs = Number.isFinite(delay) ? delay : playoutDelayMs;
        }
        if (Number.isFinite(stat.framesPerSecond)) {
          fps = stat.framesPerSecond;
        }
      }
    });
    let e2eLatencyMs = null;
    if (playoutDelayMs !== null) {
      e2eLatencyMs = playoutDelayMs + (rttMs ? rttMs / 2 : 0);
    }
    if (e2eLatencyMs === null && playoutDelayMs === null) return;
    if (e2eLatencyMs !== null) {
      stream.value.latency = `${Math.round(e2eLatencyMs)}ms`;
    }
    if (playoutDelayMs !== null) {
      stream.value.playoutDelay = `${Math.round(playoutDelayMs)}ms`;
    }
    if (fps !== null) {
      stream.value.fps = `${Math.round(fps)} fps`;
    }
    await fetch(apiUrl("/api/stream/telemetry"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        e2eLatencyMs,
        playoutDelayMs,
        fps
      })
    });
  } catch {
    // ignore
  }
};

const modeLabels = {
  auto: "智能",
  whep: "超低延迟",
  "ll-hls": "低延迟",
  hls: "稳定"
};

const availableModes = computed(() => {
  const modes = playInfo.value?.modes?.length ? playInfo.value.modes : [];
  if (!modes.length) {
    const fallback = [];
    if (playInfo.value.whep) fallback.push("whep");
    if (playInfo.value.llHls) fallback.push("ll-hls");
    if (playInfo.value.hls) fallback.push("hls");
    return fallback;
  }
  return modes;
});

const modeOptions = computed(() => ["auto", ...availableModes.value]);

const countdownText = computed(() => {
  if (!countdown.value?.next || countdown.value.remainingSecs <= 0) return "暂无排期";
  return `开播倒计时 ${formatDuration(countdown.value.remainingSecs)}`;
});

const formatDuration = (totalSecs) => {
  const secs = Math.max(0, Math.floor(totalSecs));
  const hours = String(Math.floor(secs / 3600)).padStart(2, "0");
  const minutes = String(Math.floor((secs % 3600) / 60)).padStart(2, "0");
  const seconds = String(secs % 60).padStart(2, "0");
  return `${hours}:${minutes}:${seconds}`;
};

const startVideoFps = () => {
  const video = videoRef.value;
  if (!video || typeof video.requestVideoFrameCallback !== "function") return;
  fpsFrameCount = 0;
  fpsWindowStart = performance.now();
  const onFrame = () => {
    fpsFrameCount += 1;
    const now = performance.now();
    const elapsed = now - fpsWindowStart;
    if (elapsed >= 1000) {
      const fps = (fpsFrameCount * 1000) / elapsed;
      stream.value.fps = `${Math.round(fps)} fps`;
      fpsFrameCount = 0;
      fpsWindowStart = now;
    }
    fpsHandle = video.requestVideoFrameCallback(onFrame);
  };
  fpsHandle = video.requestVideoFrameCallback(onFrame);
};

const normalizeChatMessage = (item) => {
  if (!item) return item;
  const raw = item.created_at || item.createdAt;
  const ts = raw ? Date.parse(raw.replace(" ", "T")) : Date.now();
  return { ...item, ts: Number.isNaN(ts) ? Date.now() : ts };
};

const apiBase = () => import.meta.env.VITE_API_BASE || window.location.origin;

const apiUrl = (path) => {
  if (!path) return apiBase();
  if (path.startsWith("http://") || path.startsWith("https://")) return path;
  const base = apiBase().replace(/\/$/, "");
  const suffix = path.startsWith("/") ? path : `/${path}`;
  return `${base}${suffix}`;
};

const fetchJson = async (url) => {
  const res = await fetch(apiUrl(url));
  if (!res.ok) {
    throw new Error(`请求失败: ${res.status}`);
  }
  return res.json();
};

const loadAll = async () => {
  loading.value = true;
  error.value = "";
  try {
    const [streamRes, scheduleRes, opsRes, chatRes, notifyRes] = await Promise.all([
      fetchJson("/api/stream"),
      fetchJson("/api/schedule"),
      fetchJson("/api/ops/alerts"),
      fetchJson("/api/chat/latest"),
      fetchJson("/api/notifications")
    ]);
    stream.value = streamRes;
    scheduleList.value = scheduleRes.items || [];
    opsAlerts.value = opsRes.items || [];
    chatMessages.value = (chatRes.items || []).map(normalizeChatMessage);
    notifications.value = notifyRes.items || [];
    playerError.value = "";
    const extraResults = await Promise.allSettled([
      fetchJson("/api/stream/play"),
      fetchJson("/api/stream/replay"),
      fetchJson("/api/chat/activity"),
      fetchJson("/api/chat/leaderboard"),
      fetchJson("/api/live/countdown")
    ]);
    const [playRes, replayRes, activityRes, leaderboardRes, countdownRes] = extraResults.map((r) =>
      r.status === "fulfilled" ? r.value : null
    );
    const normalized = {
      whep: normalizePlayUrl(playRes?.whep || ""),
      llHls: normalizePlayUrl(playRes?.llHls || ""),
      hls: normalizePlayUrl(playRes?.hls || ""),
      modes: playRes?.modes || []
    };
    playInfo.value = normalized;
    if (replayRes) {
      replayInfo.value = {
        enabled: !!replayRes.enabled,
        maxSeconds: replayRes.maxSeconds || 0,
        rewindSteps: replayRes.rewindSteps || [10, 30, 60],
        hls: normalizePlayUrl(replayRes.hls || "")
      };
    }
    if (activityRes) {
      chatActivity.value = {
        per10s: activityRes.per10s || 0,
        heat: activityRes.heat || "Low"
      };
    }
    if (leaderboardRes?.items) {
      leaderboard.value = leaderboardRes.items || [];
    }
    if (countdownRes) {
      countdown.value = {
        next: countdownRes.next || null,
        remainingSecs: countdownRes.remainingSecs || 0
      };
    }
    const mode = selectedMode.value;
    if (mode !== "auto" && !availableModes.value.includes(mode)) {
      selectedMode.value = "auto";
      localStorage.setItem("playback_mode", "auto");
    }
    await startPlaybackFromMode();
  } catch (err) {
    error.value = err instanceof Error ? err.message : "加载失败";
  } finally {
    loading.value = false;
  }
};

const wsUrl = () => {
  const envUrl = import.meta.env.VITE_WS_URL;
  if (envUrl) return envUrl;
  if (import.meta.env.VITE_API_BASE) {
    const base = import.meta.env.VITE_API_BASE.replace(/\/$/, "");
    const proto = base.startsWith("https://") ? "wss" : "ws";
    return `${proto}://${base.replace(/^https?:\/\//, "")}/ws`;
  }
  if (import.meta.env.DEV) return "ws://localhost:5174/ws";
  const proto = window.location.protocol === "https:" ? "wss" : "ws";
  return `${proto}://${window.location.host}/ws`;
};

const applyUpdate = (type, data) => {
  switch (type) {
    case "stream:update":
      stream.value = data;
      break;
    case "schedule:update":
      scheduleList.value = data;
      break;
    case "ops:update":
      opsAlerts.value = data;
      break;
    case "chat:new":
      chatMessages.value = [...chatMessages.value, normalizeChatMessage(data)].slice(-200);
      break;
    case "chat:activity":
      chatActivity.value = {
        per10s: data?.per10s || 0,
        heat: data?.heat || "Low"
      };
      break;
    case "notify:new":
      notifications.value = [...notifications.value, data].slice(-50);
      if (shouldNotify(data?.type)) {
        notifyBanner.value = data?.title || "有新通知";
        setTimeout(() => {
          notifyBanner.value = "";
        }, 3000);
      }
      break;
    default:
      break;
  }
};

let ws = null;
let reconnectTimer = null;
let refreshTimer = null;

const connectWs = () => {
  if (ws) {
    ws.close();
    ws = null;
  }
  wsStatus.value = "connecting";

  ws = new WebSocket(wsUrl());
  ws.onopen = () => {
    wsStatus.value = "connected";
  };
  ws.onmessage = (event) => {
    try {
      const payload = JSON.parse(event.data);
      if (payload.type === "snapshot") {
        if (payload.data?.stream) stream.value = payload.data.stream;
        if (payload.data?.schedule) scheduleList.value = payload.data.schedule;
        if (payload.data?.opsAlerts) opsAlerts.value = payload.data.opsAlerts;
        if (payload.data?.notifications) notifications.value = payload.data.notifications;
        if (payload.data?.chat?.items) chatMessages.value = payload.data.chat.items.map(normalizeChatMessage);
      } else if (payload.type && payload.data) {
        applyUpdate(payload.type, payload.data);
      }
    } catch {
      wsStatus.value = "error";
    }
  };
  ws.onclose = () => {
    wsStatus.value = "disconnected";
    if (reconnectTimer) clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(() => {
      connectWs();
    }, 3000);
  };
};

const startCountdownTimer = () => {
  if (countdownTimer) clearInterval(countdownTimer);
  countdownTimer = setInterval(() => {
    if (!countdown.value?.next) return;
    if (countdown.value.remainingSecs > 0) {
      countdown.value = {
        ...countdown.value,
        remainingSecs: Math.max(0, countdown.value.remainingSecs - 1)
      };
    }
  }, 1000);
};

const updateDrawerPosition = () => {
  if (!isMobile.value || !videoFrameRef.value) return;
  const rect = videoFrameRef.value.getBoundingClientRect();
  const minTop = Math.round(rect.top + 8);
  const baseTop = Math.round(rect.bottom - 48);
  const maxTop = window.innerHeight - 72;
  const top = Math.min(Math.max(baseTop, minTop), maxTop);
  drawerStyle.value = { top: `${top}px` };
};

const scheduleDrawerUpdate = () => {
  if (drawerRaf) cancelAnimationFrame(drawerRaf);
  drawerRaf = requestAnimationFrame(updateDrawerPosition);
};

const parseNumber = (value) => {
  if (typeof value === "number") return value;
  if (!value) return null;
  const parsed = Number.parseFloat(String(value).replace(/[^0-9.]+/g, ""));
  return Number.isNaN(parsed) ? null : parsed;
};

const parseBitrateMbps = (value) => {
  const num = parseNumber(value);
  if (num === null) return null;
  const lower = String(value).toLowerCase();
  if (lower.includes("kbps")) return num / 1000;
  return num;
};

const parseLatencyMs = (value) => {
  const num = parseNumber(value);
  if (num === null) return null;
  const lower = String(value).toLowerCase();
  if (lower.includes("s") && !lower.includes("ms")) return num * 1000;
  return num;
};

const bitrateStatus = () => {
  const mbps = parseBitrateMbps(stream.value.bitrate);
  if (mbps === null) return "unknown";
  if (mbps < 4) return "bad";
  if (mbps > 10) return "warn";
  return "ok";
};

const latencyStatus = () => {
  const ms = parseLatencyMs(stream.value.latency);
  if (ms === null) return "unknown";
  if (ms > 2500) return "bad";
  if (ms > 600) return "warn";
  return "ok";
};

const sendChat = async () => {
  if (!viewerAuthed.value) {
    showViewerAuth.value = true;
    return;
  }
  const user = (chatUser.value || "观众").trim();
  const text = chatText.value.trim();
  if (!text) return;
  chatSending.value = true;
  try {
    localStorage.setItem("viewer_name", user);
    const res = await fetch(apiUrl("/api/chat/send"), {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(viewerToken.value ? { authorization: `Bearer ${viewerToken.value}` } : {})
      },
      body: JSON.stringify({ text })
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      notifyBanner.value = data?.error || "发送失败";
      setTimeout(() => {
        notifyBanner.value = "";
      }, 2000);
      return;
    }
    chatText.value = "";
  } finally {
    chatSending.value = false;
  }
};

const createClip = async (lengthSecs) => {
  if (!viewerAuthed.value) {
    showViewerAuth.value = true;
    return;
  }
  if (clipCreating.value) return;
  clipCreating.value = true;
  clipNotice.value = "";
  lastClipUrl.value = "";
  try {
    const res = await fetch(apiUrl("/api/clip/create"), {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(viewerToken.value ? { authorization: `Bearer ${viewerToken.value}` } : {})
      },
      body: JSON.stringify({ lengthSecs })
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data?.error || "剪辑创建失败");
    }
    const rawUrl = data.clip_url || "";
    lastClipUrl.value = rawUrl.startsWith("/") ? apiUrl(rawUrl) : rawUrl;
    clipNotice.value = "剪辑已开始生成";
  } catch (err) {
    clipNotice.value = err instanceof Error ? err.message : "剪辑创建失败";
  } finally {
    clipCreating.value = false;
    if (clipNotice.value) {
      setTimeout(() => {
        clipNotice.value = "";
      }, 3000);
    }
  }
};

const loginViewer = async () => {
  if (!viewerTurnstileToken.value) {
    viewerNotice.value = "请完成人机验证";
    return;
  }
  if (!viewerUsername.value || !viewerPassword.value) {
    viewerNotice.value = "请输入账号与密码";
    return;
  }
  viewerSending.value = true;
  viewerNotice.value = "";
  try {
    const res = await fetch(apiUrl("/api/viewer/login"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        username: viewerUsername.value,
        password: viewerPassword.value,
        turnstileToken: viewerTurnstileToken.value,
        rememberDays: viewerRememberDays.value || undefined
      })
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data?.error || "登录失败");
    }
    viewerToken.value = data.token;
    viewerSessionOk.value = true;
    viewerIdentity.value = data.username || viewerUsername.value.trim();
    localStorage.setItem("viewer_token", viewerToken.value);
    localStorage.setItem("viewer_identity", viewerIdentity.value);
    if (data.expiresIn) {
      localStorage.setItem("viewer_token_expires_at", String(Date.now() + Number(data.expiresIn) * 1000));
    }
    if (viewerRememberUser.value) {
      localStorage.setItem("viewer_user", viewerUsername.value.trim());
    } else {
      localStorage.removeItem("viewer_user");
    }
    if (viewerRememberPass.value) {
      localStorage.setItem("viewer_pass", viewerPassword.value);
    } else {
      localStorage.removeItem("viewer_pass");
    }
    if (viewerRememberDays.value) {
      localStorage.setItem("viewer_remember_days", String(viewerRememberDays.value));
    } else {
      localStorage.removeItem("viewer_remember_days");
    }
    chatUser.value = viewerIdentity.value;
    await loadViewerSubscriptions();
    await loadViewerProfile();
    viewerPassword.value = "";
    viewerTurnstileToken.value = "";
    if (window.turnstile) window.turnstile.reset();
    showViewerAuth.value = false;
  } catch (err) {
    viewerNotice.value = err instanceof Error ? err.message : "登录失败";
  } finally {
    viewerSending.value = false;
    if (window.turnstile) window.turnstile.reset();
  }
};

const sendViewerCode = async () => {
  if (!viewerTurnstileToken.value) {
    viewerNotice.value = "请完成人机验证";
    return;
  }
  const ensureRegisterToken = async () => {
    if (
      viewerRegisterToken.value &&
      viewerRegisterTokenExpiresAt.value > Date.now()
    ) {
      return viewerRegisterToken.value;
    }
    const res = await fetch(apiUrl("/api/viewer/register/token"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ turnstileToken: viewerTurnstileToken.value })
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data?.error || "获取验证码令牌失败");
    }
    viewerRegisterToken.value = data.token || "";
    const expiresIn = Number(data.expiresIn || 0);
    viewerRegisterTokenExpiresAt.value = Date.now() + expiresIn * 1000;
    return viewerRegisterToken.value;
  };
  const email = viewerEmail.value.trim();
  if (!email) {
    viewerNotice.value = "请输入邮箱";
    return;
  }
  viewerSending.value = true;
  viewerNotice.value = "";
  try {
    const registerToken = await ensureRegisterToken();
    const res = await fetch(apiUrl("/api/viewer/register/start"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        email,
        registerToken
      })
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      viewerRegisterToken.value = "";
      viewerRegisterTokenExpiresAt.value = 0;
      throw new Error(data?.error || "验证码发送失败");
    }
    viewerRegisterToken.value = "";
    viewerRegisterTokenExpiresAt.value = 0;
    viewerNotice.value = "验证码已发送，请查收邮箱";
  } catch (err) {
    viewerNotice.value = err instanceof Error ? err.message : "验证码发送失败";
  } finally {
    viewerSending.value = false;
    if (window.turnstile) window.turnstile.reset();
  }
};

const registerViewer = async () => {
  if (!viewerTurnstileToken.value) {
    viewerNotice.value = "请完成人机验证";
    return;
  }
  const email = viewerEmail.value.trim();
  const username = viewerUsername.value.trim();
  const password = viewerPassword.value.trim();
  const code = viewerCode.value.trim();
  if (!email || !username || !password || !code) {
    viewerNotice.value = "请完整填写注册信息";
    return;
  }
  viewerSending.value = true;
  viewerNotice.value = "";
  try {
    const res = await fetch(apiUrl("/api/viewer/register/verify"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, username, password, code, turnstileToken: viewerTurnstileToken.value })
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data?.error || "注册失败");
    }
    viewerToken.value = data.token;
    viewerSessionOk.value = true;
    viewerIdentity.value = data.username || username;
    localStorage.setItem("viewer_token", viewerToken.value);
    localStorage.setItem("viewer_identity", viewerIdentity.value);
    if (data.expiresIn) {
      localStorage.setItem("viewer_token_expires_at", String(Date.now() + Number(data.expiresIn) * 1000));
    }
    chatUser.value = viewerIdentity.value;
    await loadViewerSubscriptions();
    await loadViewerProfile();
    viewerPassword.value = "";
    viewerCode.value = "";
    viewerTurnstileToken.value = "";
    showViewerAuth.value = false;
    if (window.turnstile) window.turnstile.reset();
  } catch (err) {
    viewerNotice.value = err instanceof Error ? err.message : "注册失败";
  } finally {
    viewerSending.value = false;
    if (window.turnstile) window.turnstile.reset();
  }
};

const loadViewerMe = async () => {
  try {
    const res = await fetch(apiUrl("/api/viewer/me"), {
      headers: {
        ...(viewerToken.value ? { authorization: `Bearer ${viewerToken.value}` } : {})
      }
    });
    if (!res.ok) {
      viewerToken.value = "";
      viewerIdentity.value = "";
      localStorage.removeItem("viewer_token");
      localStorage.removeItem("viewer_token_expires_at");
      localStorage.removeItem("viewer_identity");
      viewerSessionOk.value = false;
      return;
    }
    const data = await res.json();
    viewerIdentity.value = data.username || viewerIdentity.value;
    chatUser.value = viewerIdentity.value || chatUser.value;
    if (viewerIdentity.value) showViewerAuth.value = false;
    viewerSessionOk.value = true;
    await loadViewerProfile();
    await loadViewerSubscriptions();
  } catch {
    // ignore
  }
};

const setupMobileWatch = () => {
  if (typeof window === "undefined" || !window.matchMedia) return;
  const mq = window.matchMedia("(max-width: 768px)");
  const update = () => {
    isMobile.value = mq.matches;
  };
  update();
  if (typeof mq.addEventListener === "function") {
    mq.addEventListener("change", update);
  } else {
    mq.addListener(update);
  }
};


const loadViewerProfile = async () => {
  if (!viewerToken.value) return;
  try {
    const res = await fetch(apiUrl("/api/viewer/profile"), {
      headers: { authorization: `Bearer ${viewerToken.value}` }
    });
    if (!res.ok) return;
    const data = await res.json();
    viewerProfile.value = {
      username: data.username || viewerIdentity.value,
      email: data.email || viewerProfile.value.email
    };
  } catch {
    // ignore
  }
};

const saveViewerProfile = async () => {
  if (!viewerToken.value) return;
  viewerProfileSaving.value = true;
  viewerNotice.value = "";
  try {
    const res = await fetch(apiUrl("/api/viewer/profile"), {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        authorization: `Bearer ${viewerToken.value}`
      },
      body: JSON.stringify({
        username: viewerProfile.value.username,
        email: viewerProfile.value.email
      })
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data?.error || "保存失败");
    }
    viewerIdentity.value = data.username || viewerProfile.value.username;
    localStorage.setItem("viewer_identity", viewerIdentity.value);
    chatUser.value = viewerIdentity.value;
    viewerNotice.value = "资料已更新";
  } catch (err) {
    viewerNotice.value = err instanceof Error ? err.message : "保存失败";
  } finally {
    viewerProfileSaving.value = false;
    setTimeout(() => {
      viewerNotice.value = "";
    }, 2000);
  }
};

const logoutViewer = () => {
  fetch(apiUrl("/api/viewer/logout"), { method: "POST" }).catch(() => {});
  viewerToken.value = "";
  viewerIdentity.value = "";
  localStorage.removeItem("viewer_token");
  localStorage.removeItem("viewer_token_expires_at");
  localStorage.removeItem("viewer_identity");
  viewerSessionOk.value = false;
  showViewerProfile.value = false;
};

const waitIceGathering = (peer) => new Promise((resolve) => {
  if (peer.iceGatheringState === "complete") return resolve();
  const check = () => {
    if (peer.iceGatheringState === "complete") {
      peer.removeEventListener("icegatheringstatechange", check);
      resolve();
    }
  };
  peer.addEventListener("icegatheringstatechange", check);
});

const supportsWhep = () => typeof RTCPeerConnection !== "undefined";

const stopPlayback = () => {
  if (statsTimer) {
    clearInterval(statsTimer);
    statsTimer = null;
  }
  if (pc) {
    pc.close();
    pc = null;
  }
  if (videoRef.value) {
    videoRef.value.pause();
    videoRef.value.srcObject = null;
    if (videoRef.value.src) {
      videoRef.value.removeAttribute("src");
      videoRef.value.load();
    }
  }
};

const startHls = async (rawUrl, mode) => {
  const url = normalizePlayUrl(rawUrl || "");
  if (!url) {
    playerStatus.value = "idle";
    playerError.value = "未开播";
    return false;
  }
  stopPlayback();
  playerError.value = "";
  playerStatus.value = "connecting";
  if (videoRef.value) {
    videoRef.value.srcObject = null;
    videoRef.value.src = url;
    try {
      await videoRef.value.play();
    } catch {
      // autoplay may be blocked
    }
    showUnmutePrompt.value = videoRef.value.muted;
  }
  playerStatus.value = "playing";
  activeMode.value = mode;
  return true;
};

const requestUnmute = async () => {
  if (!videoRef.value) return;
  unmuteTried.value = true;
  videoRef.value.muted = false;
  try {
    await videoRef.value.play();
    showUnmutePrompt.value = false;
  } catch {
    showUnmutePrompt.value = true;
  }
};

const startPlaybackFromMode = async () => {
  if (!playInfo.value?.whep && !playInfo.value?.hls && !playInfo.value?.llHls) {
    playerError.value = "未开播";
    playerStatus.value = "idle";
    return;
  }
  const mode = selectedMode.value;
  if (mode === "auto") {
    if (playInfo.value.whep && supportsWhep()) {
      const ok = await startWhep(playInfo.value.whep);
      if (ok) return;
    }
    if (playInfo.value.llHls) {
      await startHls(playInfo.value.llHls, "ll-hls");
      return;
    }
    if (playInfo.value.hls) {
      await startHls(playInfo.value.hls, "hls");
    }
    return;
  }
  if (mode === "whep") {
    if (!playInfo.value.whep || !supportsWhep()) {
      playerError.value = "当前设备不支持 WHEP";
      playerStatus.value = "error";
      return;
    }
    await startWhep(playInfo.value.whep);
    return;
  }
  if (mode === "ll-hls") {
    await startHls(playInfo.value.llHls || playInfo.value.hls, "ll-hls");
    return;
  }
  await startHls(playInfo.value.hls, "hls");
};

const resolveSeekableEnd = (video) => {
  if (!video) return null;
  if (video.seekable && video.seekable.length > 0) {
    return video.seekable.end(video.seekable.length - 1);
  }
  if (Number.isFinite(video.duration)) return video.duration;
  return null;
};

const canReplay = computed(() => {
  if (!replayInfo.value.enabled) return false;
  return !!(replayInfo.value.hls || playInfo.value.llHls || playInfo.value.hls);
});

const ensureHlsForReplay = async () => {
  if (activeMode.value === "hls" || activeMode.value === "ll-hls") return true;
  replayWasWhep.value = activeMode.value === "whep";
  const hlsUrl = replayInfo.value.hls || playInfo.value.llHls || playInfo.value.hls;
  if (!hlsUrl) return false;
  await startHls(hlsUrl, playInfo.value.llHls ? "ll-hls" : "hls");
  if (replayWasWhep.value) {
    replayNotice.value = "已进入回放模式（已切换为 HLS）";
    setTimeout(() => {
      replayNotice.value = "";
    }, 2500);
  }
  return true;
};

const rewindPlayback = async (seconds) => {
  if (!(await ensureHlsForReplay())) return;
  const video = videoRef.value;
  const end = resolveSeekableEnd(video);
  if (end === null) return;
  const target = Math.max(0, end - seconds);
  video.currentTime = target;
};

const jumpLive = async () => {
  if (!(await ensureHlsForReplay())) return;
  const video = videoRef.value;
  const end = resolveSeekableEnd(video);
  if (end === null) return;
  video.currentTime = end;
  if (replayWasWhep.value && playInfo.value.whep && supportsWhep()) {
    replayWasWhep.value = false;
    await startWhep(playInfo.value.whep);
    replayNotice.value = "已返回直播（切回 WHEP）";
    setTimeout(() => {
      replayNotice.value = "";
    }, 2500);
  }
};

const startWhep = async (url) => {
  if (!url) return;
  playerError.value = "";
  playerStatus.value = "connecting";
  stopPlayback();

  pc = new RTCPeerConnection();
  pc.addTransceiver("video", { direction: "recvonly" });
  pc.addTransceiver("audio", { direction: "recvonly" });

  pc.ontrack = (event) => {
    const stream = event.streams[0];
    if (videoRef.value && stream) {
      videoRef.value.srcObject = stream;
      startVideoFps();
      videoRef.value.play().catch(() => {});
      showUnmutePrompt.value = videoRef.value.muted;
    }
  };

  const offer = await pc.createOffer();
  await pc.setLocalDescription(offer);
  await waitIceGathering(pc);

  let res = null;
  try {
    res = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/sdp", Accept: "application/sdp" },
      body: pc.localDescription.sdp
    });
  } catch {
    playerStatus.value = "idle";
      playerError.value = "未开播";
    return false;
  }

  if (!res.ok) {
    const text = await res.text();
    if (res.status === 404 && text.includes("no stream is available")) {
      playerStatus.value = "idle";
      playerError.value = "未开播";
      return false;
    }
    playerStatus.value = "error";
    playerError.value = `WHEP 连接失败 ${res.status}: ${text || "Bad Request"}`;
    return false;
  }

  const answerSdp = await res.text();
  if (!answerSdp) {
    playerStatus.value = "error";
    playerError.value = "WHEP 无返回 SDP";
    return false;
  }
  await pc.setRemoteDescription({ type: "answer", sdp: answerSdp });
  playerStatus.value = "playing";
  activeMode.value = "whep";
  statsTimer = setInterval(reportWhepStats, 5000);
  return true;
};

onMounted(() => {
  loadAll();
  connectWs();
  loadViewerMe();
  ensureTurnstileScript();
  setupMobileWatch();
  startCountdownTimer();
  nextTick(() => {
    updateDrawerPosition();
  });
  window.onViewerTurnstile = (token) => {
    viewerTurnstileToken.value = token;
  };
  if (typeof window !== "undefined" && window.matchMedia) {
    collapseMedia = window.matchMedia("(max-width: 1400px)");
    collapseListener = () => {
      isCollapsed.value = collapseMedia.matches;
    };
    collapseListener();
    collapseMedia.addEventListener("change", collapseListener);
  }
  nextTick(() => {
    scrollChatToBottom();
  });
  refreshTimer = setInterval(() => {
    if (wsStatus.value !== "connected") {
      loadAll();
    }
  }, 20000);
  window.addEventListener("resize", scheduleDrawerUpdate);
  window.addEventListener("scroll", scheduleDrawerUpdate, true);
  leaderboardTimer = setInterval(async () => {
    try {
      const data = await fetchJson("/api/chat/leaderboard");
      leaderboard.value = data.items || [];
    } catch {
      // ignore
    }
  }, 30000);
  activityTimer = setInterval(async () => {
    try {
      const data = await fetchJson("/api/chat/activity");
      chatActivity.value = { per10s: data.per10s || 0, heat: data.heat || "Low" };
    } catch {
      // ignore
    }
  }, 10000);
});

onBeforeUnmount(() => {
  if (reconnectTimer) clearTimeout(reconnectTimer);
  if (refreshTimer) clearInterval(refreshTimer);
  if (leaderboardTimer) clearInterval(leaderboardTimer);
  if (activityTimer) clearInterval(activityTimer);
  if (drawerRaf) cancelAnimationFrame(drawerRaf);
  window.removeEventListener("resize", scheduleDrawerUpdate);
  window.removeEventListener("scroll", scheduleDrawerUpdate, true);
  if (countdownTimer) clearInterval(countdownTimer);
  if (ws) ws.close();
  if (pc) pc.close();
  if (statsTimer) clearInterval(statsTimer);
  if (fpsHandle) cancelAnimationFrame(fpsHandle);
  if (resizeObserver) resizeObserver.disconnect();
  if (collapseMedia && collapseListener) {
    collapseMedia.removeEventListener("change", collapseListener);
  }
});

const shouldNotify = (type) => {
  if (type === "schedule") return viewerSubscriptions.value.schedule;
  return false;
};

const loadViewerSubscriptions = async () => {
  if (!viewerToken.value) return;
  try {
    const res = await fetch(apiUrl("/api/viewer/subscribe"), {
      headers: { authorization: `Bearer ${viewerToken.value}` }
    });
    if (!res.ok) return;
    const data = await res.json();
    viewerSubscriptions.value = {
      live: !!data.live,
      schedule: !!data.schedule,
      email: !!data.email
    };
  } catch {
    // ignore
  }
};

const saveViewerSubscriptions = async () => {
  if (!viewerToken.value) {
    showViewerAuth.value = true;
    return;
  }
  try {
    const res = await fetch(apiUrl("/api/viewer/subscribe"), {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        authorization: `Bearer ${viewerToken.value}`
      },
      body: JSON.stringify({
        live: viewerSubscriptions.value.live,
        schedule: viewerSubscriptions.value.schedule,
        email: viewerSubscriptions.value.email
      })
    });
    if (!res.ok) return;
    const data = await res.json();
    viewerSubscriptions.value = {
      live: !!data.live,
      schedule: !!data.schedule,
      email: !!data.email
    };
    notifyBanner.value = "订阅已更新";
    setTimeout(() => {
      notifyBanner.value = "";
    }, 2000);
  } catch {
    // ignore
  }
};

const ensureTurnstileScript = () => {
  if (window.turnstile) return Promise.resolve();
  if (turnstileScriptLoading) return Promise.resolve();
  turnstileScriptLoading = true;
  return new Promise((resolve) => {
    const existing = document.getElementById("turnstile-script");
    if (existing) {
      existing.addEventListener("load", () => resolve());
      return;
    }
    const script = document.createElement("script");
    script.id = "turnstile-script";
    script.src = "https://challenges.cloudflare.com/turnstile/v0/api.js";
    script.async = true;
    script.defer = true;
    script.onload = () => resolve();
    document.body.appendChild(script);
  });
};

const renderViewerTurnstile = async () => {
  await nextTick();
  await ensureTurnstileScript();
  if (!window.turnstile) return;
  if (turnstileLoginId) {
    window.turnstile.remove(turnstileLoginId);
    turnstileLoginId = null;
  }
  if (turnstileRegisterId) {
    window.turnstile.remove(turnstileRegisterId);
    turnstileRegisterId = null;
  }
  if (turnstileLoginRef.value) {
    turnstileLoginId = window.turnstile.render(turnstileLoginRef.value, {
      sitekey: "0x4AAAAAACncUgjk6YpyY6aB",
      callback: (token) => {
        viewerTurnstileToken.value = token;
      }
    });
  }
  if (turnstileRegisterRef.value) {
    turnstileRegisterId = window.turnstile.render(turnstileRegisterRef.value, {
      sitekey: "0x4AAAAAACncUgjk6YpyY6aB",
      callback: (token) => {
        viewerTurnstileToken.value = token;
      }
    });
  }
};

watch([showViewerAuth, viewerMode], () => {
  if (showViewerAuth.value) renderViewerTurnstile();
});


watch(chatMessages, () => {
  nextTick(() => {
    if (chatAtBottom.value) {
      scrollChatToBottom();
    } else {
      chatHasNew.value = true;
    }
  });
});

watch(playInfo, () => {
  if (playInfo.value?.hls && videoRef.value) {
    startVideoFps();
  }
});

watch(selectedMode, () => {
  localStorage.setItem("playback_mode", selectedMode.value);
  startPlaybackFromMode();
});

watch(countdown, () => {
  if (!countdownTimer) startCountdownTimer();
});

watch(isMobile, () => {
  if (!isMobile.value) {
    showMobileControls.value = false;
    replayExpanded.value = false;
    showMobileDrawer.value = false;
  } else {
    nextTick(() => {
      updateDrawerPosition();
    });
  }
});
</script>

<template>
  <div
    class="app-container font-body transition-colors duration-700 ease-in-out"
    :class="isNight ? 'app-night text-meow-night-ink' : 'app-day text-meow-ink'"
  >
    <header class="app-header">
      <div class="flex items-center gap-3">
        <img
          src="/logo.png"
          alt="Meowhuan logo"
          class="h-10 w-10 rounded-full border bg-white/70 object-cover shadow-sm"
          :class="isNight ? 'border-meow-night-line' : 'border-meow-line'"
        />
        <div>
          <div class="font-display text-xl tracking-wide">喵喵直播间</div>
          <div class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">欢迎来到观众端</div>
        </div>
      </div>
      <div class="flex items-center gap-2 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
        <button
          class="meow-pill motion-press hidden md:inline-flex"
          type="button"
          @click="overlayTab = 'notify'"
        >
          关注
        </button>
        <span class="status-pill" :class="wsStatus === 'connected' ? 'status-live' : 'status-idle'">
          <span class="status-dot"></span>
          {{ wsStatus === "connected" ? "实时连接" : "同步中" }}
        </span>
        <button class="meow-pill motion-press" type="button" @click="isNight = !isNight">
          {{ isNight ? "夜间" : "白天" }}
        </button>
        <button v-if="!viewerAuthed" class="meow-pill motion-press" type="button" @click="showViewerAuth = true">
          登录
        </button>
        <button v-else class="meow-pill motion-press" type="button" @click="showViewerProfile = true">
          {{ viewerIdentity || "用户" }}
        </button>
      </div>
    </header>
    <div v-if="notifyBanner" class="notify-banner">
      {{ notifyBanner }}
    </div>

    <div class="app-main" :style="{ '--sidebar-width': isCollapsed ? '60px' : '280px' }">
      <aside class="left-sidebar" :class="isCollapsed ? 'is-collapsed' : ''">
        <div class="sidebar-actionbar">
          <span class="font-display text-sm">互动小窗</span>
          <button v-if="!isCollapsed" class="meow-pill motion-press sidebar-toggle" type="button" @click="isCollapsed = true" aria-label="收起">
            ←
          </button>
        </div>
        <div v-if="!isCollapsed" class="sidebar-body">
          <div class="sidebar-tabs">
            <button class="meow-pill motion-press" :class="overlayTab === 'schedule' ? 'info-tab-active' : ''" type="button" @click="overlayTab = 'schedule'">排期</button>
            <button v-if="!isMobile" class="meow-pill motion-press" :class="overlayTab === 'playback' ? 'info-tab-active' : ''" type="button" @click="overlayTab = 'playback'">播放</button>
            <button class="meow-pill motion-press" :class="overlayTab === 'tasks' ? 'info-tab-active' : ''" type="button" @click="overlayTab = 'tasks'">任务</button>
            <button class="meow-pill motion-press" :class="overlayTab === 'rank' ? 'info-tab-active' : ''" type="button" @click="overlayTab = 'rank'">榜单</button>
            <button class="meow-pill motion-press" :class="overlayTab === 'ops' ? 'info-tab-active' : ''" type="button" @click="overlayTab = 'ops'">运营</button>
            <button class="meow-pill motion-press" :class="overlayTab === 'notify' ? 'info-tab-active' : ''" type="button" @click="overlayTab = 'notify'">通知</button>
          </div>
          <div v-if="overlayTab === 'schedule'" class="info-panel compact-list">
            <div v-for="item in scheduleList" :key="item.title" class="meow-window-item">
              <div class="meow-window-time" :title="item.time">
                {{ item.time }}
                <span class="meow-pill schedule-tag" :title="item.tag">{{ item.tag }}</span>
              </div>
              <div>
                <div class="meow-window-titleline">
                  <span :title="item.title">{{ item.title }}</span>
                </div>
                <div class="meow-window-meta" :title="`主持：${item.host}`">主持：{{ item.host }}</div>
              </div>
            </div>
          </div>
          <div v-else-if="overlayTab === 'playback'" class="info-panel playback-panel">
            <div class="playback-title">播放控制</div>
            <div class="player-controls">
              <div class="mode-group">
                <span class="mode-label">延迟模式</span>
                <div class="mode-buttons">
                  <button
                    v-for="mode in modeOptions"
                    :key="mode"
                    class="meow-pill motion-press"
                    :class="selectedMode === mode ? 'info-tab-active' : ''"
                    type="button"
                    @click="selectedMode = mode"
                  >
                    {{ modeLabels[mode] || mode }}
                  </button>
                </div>
              </div>
              <div class="replay-group">
                <span class="mode-label">回放缓冲</span>
                <div class="mode-buttons">
                  <button
                    v-for="step in replayInfo.rewindSteps"
                    :key="`rewind-${step}`"
                    class="meow-pill motion-press"
                    :disabled="!canReplay"
                    type="button"
                    @click="rewindPlayback(step)"
                  >
                    回退 {{ step }}s
                  </button>
                  <button
                    class="meow-pill motion-press"
                    :disabled="!canReplay"
                    type="button"
                    @click="jumpLive"
                  >
                    回到直播
                  </button>
                </div>
              </div>
              <div class="clip-group">
                <span class="mode-label">剪辑</span>
                <div class="mode-buttons">
                  <button class="meow-pill motion-press" type="button" :disabled="clipCreating || stream.status !== 'live'" @click="createClip(15)">15s</button>
                  <button class="meow-pill motion-press" type="button" :disabled="clipCreating || stream.status !== 'live'" @click="createClip(20)">20s</button>
                  <button class="meow-pill motion-press" type="button" :disabled="clipCreating || stream.status !== 'live'" @click="createClip(30)">30s</button>
                  <span v-if="clipNotice" class="clip-note">{{ clipNotice }}</span>
                  <a v-if="lastClipUrl" class="clip-link" :href="lastClipUrl" target="_blank" rel="noreferrer">打开剪辑</a>
                </div>
              </div>
            </div>
          </div>
          <div v-else-if="overlayTab === 'tasks'" class="info-panel text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
            <div>🌸 发一句今晚的心情</div>
            <div>🎧 点一首温柔的歌</div>
            <div>🌟 选择本场的封面氛围</div>
            <div class="mt-2 flex flex-wrap gap-2">
              <a
                class="meow-btn-primary motion-press"
                :class="isNight ? 'bg-meow-night-accent text-meow-night-bg' : ''"
                href="https://www.meowra.cn/donate"
                target="_blank"
                rel="noreferrer"
              >
                加入应援
              </a>
              <button class="meow-btn-ghost motion-press" :class="isNight ? 'border-meow-night-line text-meow-night-ink hover:bg-meow-night-card/80' : ''">打开弹幕</button>
            </div>
          </div>
          <div v-else-if="overlayTab === 'notify'" class="info-panel compact-list">
            <div class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">关注设置</div>
            <div class="mt-2 flex flex-wrap gap-2">
              <button class="meow-pill motion-press" :class="viewerSubscriptions.schedule ? 'info-tab-active' : ''" type="button" @click="viewerSubscriptions.schedule = !viewerSubscriptions.schedule; saveViewerSubscriptions()">
                排期通知
              </button>
              <button class="meow-pill motion-press" :class="viewerSubscriptions.email ? 'info-tab-active' : ''" type="button" @click="viewerSubscriptions.email = !viewerSubscriptions.email; saveViewerSubscriptions()">
                邮件通知
              </button>
            </div>
            <div class="mt-3 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">Telegram 频道</div>
            <div class="mt-2 flex flex-wrap gap-2 text-xs">
              <a
                class="copy-btn"
                href="https://t.me/Meowhuan_little_nest"
                target="_blank"
                rel="noreferrer"
              >
                加入 @Meowhuan_little_nest
              </a>
            </div>
          </div>
          <div v-else-if="overlayTab === 'rank'" class="info-panel compact-list">
            <div class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">活跃观众榜</div>
            <div v-if="!leaderboard.length" class="text-xs mt-3" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
              暂无数据
            </div>
            <div v-for="item in leaderboard" :key="item.user" class="meow-window-item">
              <div class="meow-window-time">#{{ item.rank }}</div>
              <div>
                <div class="meow-window-titleline">
                  <span>{{ item.user }}</span>
                </div>
                <div class="meow-window-meta">总弹幕 {{ item.messages }} · 本场 {{ item.messages_live }}</div>
              </div>
            </div>
          </div>
          <div v-else class="info-panel">
            <div v-for="alert in opsAlerts" :key="alert.title" class="stat-tile">
              <div class="text-sm font-600">{{ alert.title }}</div>
              <p class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">{{ alert.desc }}</p>
            </div>
          </div>
        </div>
        <div v-else class="sidebar-collapsed">
          <button class="collapsed-icon" type="button" @click="isCollapsed = false" aria-label="展开">☰</button>
          <div class="collapsed-times">
            <div v-for="item in scheduleList.slice(0, 5)" :key="item.time" class="collapsed-time">{{ item.time }}</div>
          </div>
        </div>
      </aside>

      <main class="main-player">
        <div class="video-shell">
          <div class="video-header">
            <div>
              <div class="text-xs uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">正在播放</div>
              <h1 class="mt-2 font-display text-3xl">{{ stream.title }}</h1>
              <div class="mt-2 text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                主播 {{ stream.host }} · 房间 {{ stream.roomId }} · {{ stream.resolution }}
              </div>
              <div class="mt-2 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                {{ countdownText }}
              </div>
            </div>
            <div class="flex items-center gap-2">
              <button
                class="meow-pill motion-press md:hidden"
                type="button"
                @click="overlayTab = 'notify'; isCollapsed = false"
              >
                关注
              </button>
              <span class="status-lamp" :class="stream.status === 'live' ? 'status-live' : (stream.status === 'ready' ? 'status-warn' : 'status-idle')"></span>
              <span class="text-sm">{{ stream.status === 'live' ? '直播中' : (stream.status === 'ready' ? '准备中' : '离线') }}</span>
            </div>
          </div>
          <div class="video-frame" ref="videoFrameRef">
          <div class="viewer-screen" :class="isNight ? 'viewer-screen-night' : ''">
            <template v-if="streamUnavailable">
              <div class="viewer-screen-inner">
                <div class="viewer-screen-badge">Live</div>
                <div class="viewer-screen-title">未开播</div>
                <div class="viewer-screen-meta">主播暂未开始直播</div>
              </div>
            </template>
            <template v-else-if="playInfo.whep">
              <video ref="videoRef" class="viewer-video" autoplay playsinline muted controls></video>
            </template>
            <template v-else-if="playInfo.hls">
              <video ref="videoRef" class="viewer-video" controls autoplay muted playsinline :src="playInfo.hls"></video>
            </template>
            <div v-else class="viewer-screen-inner">
              <div class="viewer-screen-badge">Live</div>
              <div class="viewer-screen-title">等待播放器地址</div>
              <div class="viewer-screen-meta">请在后端配置播放地址</div>
            </div>
          </div>
          <div v-if="showUnmutePrompt" class="unmute-banner">
            <div class="unmute-content">
              <div class="unmute-title">浏览器默认关闭声音</div>
              <div class="unmute-desc">点击即可开启声音播放</div>
            </div>
            <button class="meow-pill motion-press" type="button" @click="requestUnmute">
              开启声音
            </button>
          </div>
          </div>
          <div v-if="isMobile" class="mobile-drawer-anchor">
            <div class="mobile-drawer" :style="drawerStyle">
              <button class="mobile-drawer-toggle" type="button" @click="showMobileDrawer = !showMobileDrawer">
                {{ showMobileDrawer ? "›" : "‹" }}
              </button>
              <div class="mobile-drawer-panel" :class="showMobileDrawer ? 'is-open' : ''">
                <div class="mobile-controls">
                  <div class="mobile-replay">
                    <button class="meow-pill motion-press" type="button" @click="replayExpanded = !replayExpanded">
                      回放缓冲 {{ replayExpanded ? "收起" : "展开" }}
                    </button>
                    <div v-if="replayExpanded" class="mobile-replay-panel">
                      <button
                        v-for="step in replayInfo.rewindSteps"
                        :key="`m-rewind-${step}`"
                        class="meow-pill motion-press"
                        :disabled="!canReplay"
                        type="button"
                        @click="rewindPlayback(step)"
                      >
                        回退 {{ step }}s
                      </button>
                      <button
                        class="meow-pill motion-press"
                        :disabled="!canReplay"
                        type="button"
                        @click="jumpLive"
                      >
                        回到直播
                      </button>
                    </div>
                  </div>
                  <button class="meow-pill motion-press" type="button" @click="showMobileControls = true">
                    功能面板
                  </button>
                </div>
              </div>
            </div>
          </div>
          <div v-if="replayNotice" class="replay-toast">{{ replayNotice }}</div>
          <div class="mobile-controls-placeholder"></div>
          <div v-if="playerError && !streamUnavailable" class="player-error mt-2 text-xs text-[#e06b8b]">{{ playerError }}</div>
          <div class="metrics-bar">
            <span>在线观众 {{ stream.viewers }}</span>
            <span class="metric-pill" :class="`metric-${bitrateStatus()}`">码率 {{ stream.bitrate }}</span>
            <span class="metric-pill" :class="`metric-${latencyStatus()}`">端到端 {{ stream.latency }}</span>
            <span class="metric-pill">推流延迟 {{ stream.pushLatency || "-" }}</span>
            <span class="metric-pill">播放延迟 {{ stream.playoutDelay || "-" }}</span>
            <span class="metric-pill">当前帧率 {{ stream.fps || "-" }}</span>
            <span class="metric-pill">弹幕节奏 {{ chatRateLocal }}</span>
            <span class="metric-pill">聊天热度 {{ chatActivity.heat }} · {{ chatActivity.per10s }}/10s</span>
            <span class="metrics-copy">© 2026 Meowhuan Live Room</span>
          </div>
        </div>
      </main>

      <aside class="chat-panel">
        <div class="flex items-center justify-between">
          <h3 class="font-display text-lg">互动弹幕</h3>
          <span class="meow-pill">实时</span>
        </div>
        <div class="mt-2 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
          实时弹幕节奏 {{ chatRateLocal }} · 热度 {{ chatActivity.heat }}
        </div>
        <div ref="chatScrollRef" class="chat-list mt-3" @scroll="onChatScroll">
          <transition-group ref="chatListRef" name="chat-slide" tag="div">
            <div v-for="item in displayedMessages" :key="item.id || `${item.user}-${item.text}`" class="chat-item">
              <div class="chat-meta">
                <span
                  v-if="resolveLevelLabel(item.level_label)"
                  class="chat-level"
                  :class="resolveLevelClass(item.level_label)"
                >{{ resolveLevelLabel(item.level_label) }}</span>
                <button class="chat-user" type="button" @click="addMention(item.user)">{{ item.user }}</button>
              </div>
              <span class="chat-text" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'" v-html="highlightMentions(item.text)"></span>
            </div>
          </transition-group>
        </div>
        <button v-if="chatHasNew" class="chat-new-tip" type="button" @click="scrollChatToBottom">
          下方有新消息
        </button>
        <div class="chat-emoji-bar">
          <button class="emoji-btn" type="button" @click="appendEmoji(':cat:')">:cat:</button>
          <button class="emoji-btn" type="button" @click="appendEmoji(':awawa:')">:awawa:</button>
          <button class="emoji-btn" type="button" @click="appendEmoji(':thonk:')">:thonk:</button>
        </div>
        <div class="chat-input chat-input-footer" :class="viewerAuthed ? '' : 'is-locked'">
          <input v-model="chatText" class="field-input" placeholder="发送一条弹幕..." :disabled="!viewerAuthed" @keyup.enter="sendChat" />
          <button class="chat-send-btn motion-press" :disabled="chatSending || !viewerAuthed" @click="sendChat">
            发送
          </button>
          <div v-if="!viewerAuthed" class="chat-lock">
            <span>登录后发送弹幕</span>
            <button class="meow-pill motion-press" type="button" @click="showViewerAuth = true">去登录</button>
          </div>
        </div>
      </aside>
    </div>
  </div>


  <div v-if="isMobile && overlayTab === 'notify'" class="mobile-notify-sheet md:hidden">
    <div class="mobile-notify-card" :class="isNight ? 'mobile-notify-night' : 'mobile-notify-day'">
      <div class="flex items-center justify-between">
        <div class="font-display text-lg">关注与通知</div>
        <button class="meow-pill motion-press" type="button" @click="overlayTab = 'schedule'">关闭</button>
      </div>
      <div class="mt-4 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">关注设置</div>
      <div class="mt-2 flex flex-wrap gap-2">
        <button class="meow-pill motion-press" :class="viewerSubscriptions.schedule ? 'info-tab-active' : ''" type="button" @click="viewerSubscriptions.schedule = !viewerSubscriptions.schedule; saveViewerSubscriptions()">
          排期通知
        </button>
        <button class="meow-pill motion-press" :class="viewerSubscriptions.email ? 'info-tab-active' : ''" type="button" @click="viewerSubscriptions.email = !viewerSubscriptions.email; saveViewerSubscriptions()">
          邮件通知
        </button>
      </div>
      <div class="mt-3 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">Telegram 频道</div>
      <div class="mt-2 flex flex-wrap gap-2 text-xs">
        <a
          class="copy-btn"
          href="https://t.me/Meowhuan_little_nest"
          target="_blank"
          rel="noreferrer"
        >
          加入 @Meowhuan_little_nest
        </a>
      </div>
    </div>
  </div>

  <div v-if="isMobile && showMobileControls" class="mobile-control-sheet md:hidden">
    <div class="mobile-control-card" :class="isNight ? 'mobile-notify-night' : 'mobile-notify-day'">
      <div class="flex items-center justify-between">
        <div class="font-display text-lg">播放功能面板</div>
        <button class="meow-pill motion-press" type="button" @click="showMobileControls = false">关闭</button>
      </div>
      <div class="mt-4 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">延迟模式</div>
      <div class="mt-2 flex flex-wrap gap-2">
        <button
          v-for="mode in modeOptions"
          :key="`m-mode-${mode}`"
          class="meow-pill motion-press"
          :class="selectedMode === mode ? 'info-tab-active' : ''"
          type="button"
          @click="selectedMode = mode"
        >
          {{ modeLabels[mode] || mode }}
        </button>
      </div>
      <div class="mt-4 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">剪辑</div>
      <div class="mt-2 flex flex-wrap gap-2">
        <button class="meow-pill motion-press" type="button" :disabled="clipCreating || stream.status !== 'live'" @click="createClip(15)">15s</button>
        <button class="meow-pill motion-press" type="button" :disabled="clipCreating || stream.status !== 'live'" @click="createClip(20)">20s</button>
        <button class="meow-pill motion-press" type="button" :disabled="clipCreating || stream.status !== 'live'" @click="createClip(30)">30s</button>
      </div>
      <div v-if="clipNotice" class="mt-2 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">{{ clipNotice }}</div>
      <a v-if="lastClipUrl" class="clip-link mt-2 inline-flex" :href="lastClipUrl" target="_blank" rel="noreferrer">打开剪辑</a>
    </div>
  </div>


  <div v-if="showViewerAuth" class="auth-overlay">
    <div class="auth-card" :class="isNight ? 'auth-card-night' : 'auth-card-day'">
      <div class="flex items-center justify-between">
        <div>
          <div class="text-xs uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">Viewer Access</div>
          <div class="font-display text-2xl mt-1">观众登录</div>
        </div>
        <button class="meow-pill motion-press" type="button" @click="showViewerAuth = false">关闭</button>
      </div>

      <div class="mt-6 flex gap-2 auth-tabs">
        <button class="meow-pill motion-press" :class="viewerMode === 'login' ? 'auth-tab-active' : ''" type="button" @click="viewerMode = 'login'">登录</button>
        <button class="meow-pill motion-press" :class="viewerMode === 'register' ? 'auth-tab-active' : ''" type="button" @click="viewerMode = 'register'">注册</button>
      </div>

      <div class="mt-6 grid gap-4" v-if="viewerMode === 'login'">
        <div class="field-group">
          <label class="field-label">账号 / 邮箱</label>
          <input v-model="viewerUsername" class="field-input" placeholder="meow_user 或 meow@example.com" />
        </div>
        <div class="field-group">
          <label class="field-label">密码</label>
          <input v-model="viewerPassword" class="field-input" type="password" placeholder="••••••••" />
        </div>
        <div class="field-group">
          <label class="field-label">登录保持</label>
          <div class="flex flex-wrap items-center gap-3 text-xs">
            <label class="meow-pill motion-press">
              <input type="checkbox" class="mr-2" v-model="viewerRememberUser" />
              记住账号
            </label>
            <label class="meow-pill motion-press">
              <input type="checkbox" class="mr-2" v-model="viewerRememberPass" />
              记住密码（仅本机）
            </label>
            <select v-model="viewerRememberDays" class="field-input !h-8 !text-xs w-[140px]">
              <option :value="0">默认时长</option>
              <option :value="7">保持 7 天</option>
              <option :value="30">保持 30 天</option>
            </select>
          </div>
        </div>
          <div class="field-group">
            <label class="field-label">人机验证</label>
            <div ref="turnstileLoginRef"></div>
          </div>
        <div class="flex flex-wrap gap-2">
          <button class="chat-send-btn motion-press" type="button" :disabled="viewerSending" @click="loginViewer">登录</button>
        </div>
      </div>

      <div class="mt-6 grid gap-4" v-else>
        <div class="field-group">
          <label class="field-label">邮箱</label>
          <div class="flex flex-wrap gap-2">
            <input v-model="viewerEmail" class="field-input flex-1 min-w-[200px]" placeholder="meow@example.com" />
            <button class="meow-btn-ghost motion-press" type="button" :disabled="viewerSending" @click="sendViewerCode">发送验证码</button>
          </div>
        </div>
        <div class="field-group">
          <label class="field-label">验证码</label>
          <input v-model="viewerCode" class="field-input" placeholder="6 位验证码" />
        </div>
        <div class="field-group">
          <label class="field-label">用户名</label>
          <input v-model="viewerUsername" class="field-input" placeholder="meow_user" />
        </div>
        <div class="field-group">
          <label class="field-label">设置密码</label>
          <input v-model="viewerPassword" class="field-input" type="password" placeholder="至少 6 位" />
        </div>
        <div class="field-group">
          <label class="field-label">人机验证</label>
          <div ref="turnstileRegisterRef"></div>
        </div>
        <div class="flex flex-wrap gap-2">
          <button class="chat-send-btn motion-press" type="button" :disabled="viewerSending" @click="registerViewer">完成注册</button>
        </div>
      </div>

      <div v-if="viewerNotice" class="mt-3 text-xs text-[#e06b8b]">{{ viewerNotice }}</div>
    </div>
  </div>

  <div v-if="showViewerProfile" class="auth-overlay">
    <div class="auth-card" :class="isNight ? 'auth-card-night' : 'auth-card-day'">
      <div class="flex items-center justify-between">
        <div>
          <div class="text-xs uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">Profile</div>
          <div class="font-display text-2xl mt-1">观众资料</div>
        </div>
        <button class="meow-pill motion-press" type="button" @click="showViewerProfile = false">关闭</button>
      </div>
      <div class="mt-6 grid gap-4">
        <div class="field-group">
          <label class="field-label">用户名</label>
          <input v-model="viewerProfile.username" class="field-input" placeholder="meow_user" />
        </div>
        <div class="field-group">
          <label class="field-label">邮箱</label>
          <input v-model="viewerProfile.email" class="field-input" placeholder="meow@example.com" />
        </div>
        <div class="flex flex-wrap gap-2">
          <button class="chat-send-btn motion-press" type="button" :disabled="viewerProfileSaving" @click="saveViewerProfile">
            保存
          </button>
          <button class="meow-pill motion-press" type="button" @click="logoutViewer">
            退出登录
          </button>
        </div>
      </div>
      <div v-if="viewerNotice" class="mt-3 text-xs text-[#e06b8b]">{{ viewerNotice }}</div>
    </div>
  </div>
</template>
