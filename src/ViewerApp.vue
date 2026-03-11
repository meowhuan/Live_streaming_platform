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
  chatRate: "-",
  viewers: 0,
  updatedAt: ""
});

const scheduleList = ref([]);
const opsAlerts = ref([]);

const chatMessages = ref([]);
const viewerToken = ref(localStorage.getItem("viewer_token") || "");
const viewerIdentity = ref(localStorage.getItem("viewer_identity") || "");
const viewerAuthed = computed(() => !!viewerToken.value);
const viewerMode = ref("login");
const viewerEmail = ref("");
const viewerUsername = ref("");
const viewerPassword = ref("");
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

const playInfo = ref({ whep: "", hls: "" });
const playerError = ref("");
const streamUnavailable = computed(() => {
  const message = playerError.value || "";
  return message.includes("no stream is available on path") || message.includes("未开播");
});
const playerStatus = ref("idle");
const videoRef = ref(null);
const chatListRef = ref(null);
const chatScrollRef = ref(null);
let pc = null;
let statsTimer = null;
let resizeObserver = null;
let collapseMedia = null;
let collapseListener = null;
const overlayTab = ref("schedule");
const isCollapsed = ref(false);
const chatAtBottom = ref(true);
const chatHasNew = ref(false);

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


const reportWhepStats = async () => {
  if (!pc || pc.connectionState === "closed") return;
  try {
    const stats = await pc.getStats();
    let rttMs = null;
    let playoutDelayMs = null;
    stats.forEach((stat) => {
      if (stat.type === "candidate-pair" && stat.state === "succeeded" && stat.currentRoundTripTime) {
        rttMs = stat.currentRoundTripTime * 1000;
      }
      if (stat.type === "inbound-rtp" && stat.kind === "video" && stat.jitterBufferEmittedCount) {
        const delay = (stat.jitterBufferDelay / stat.jitterBufferEmittedCount) * 1000;
        playoutDelayMs = Number.isFinite(delay) ? delay : playoutDelayMs;
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
    await fetch("/api/stream/telemetry", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        e2eLatencyMs,
        playoutDelayMs
      })
    });
  } catch {
    // ignore
  }
};

const normalizeChatMessage = (item) => {
  if (!item) return item;
  const raw = item.created_at || item.createdAt;
  const ts = raw ? Date.parse(raw.replace(" ", "T")) : Date.now();
  return { ...item, ts: Number.isNaN(ts) ? Date.now() : ts };
};

const fetchJson = async (url) => {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`请求失败: ${res.status}`);
  }
  return res.json();
};

const loadAll = async () => {
  loading.value = true;
  error.value = "";
  try {
    const [streamRes, scheduleRes, opsRes, chatRes] = await Promise.all([
      fetchJson("/api/stream"),
      fetchJson("/api/schedule"),
      fetchJson("/api/ops/alerts"),
      fetchJson("/api/chat/latest")
    ]);
    stream.value = streamRes;
    scheduleList.value = scheduleRes.items || [];
    opsAlerts.value = opsRes.items || [];
    chatMessages.value = (chatRes.items || []).map(normalizeChatMessage);
    playerError.value = "";
    let playRes = null;
    try {
      playRes = await fetchJson("/api/stream/play");
    } catch {
      playRes = null;
    }
    const normalized = {
      whep: normalizePlayUrl(playRes?.whep || ""),
      hls: normalizePlayUrl(playRes?.hls || "")
    };
    playInfo.value = normalized;
    if (!playInfo.value.whep && !playInfo.value.hls) {
      playerError.value = "未开播";
      playerStatus.value = "idle";
    } else if (playInfo.value.whep && playerStatus.value === "idle") {
      startWhep(playInfo.value.whep).catch((err) => {
        playerError.value = err instanceof Error ? err.message : "播放器错误";
        playerStatus.value = "error";
      });
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : "加载失败";
  } finally {
    loading.value = false;
  }
};

const wsUrl = () => {
  const envUrl = import.meta.env.VITE_WS_URL;
  if (envUrl) return envUrl;
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
    const res = await fetch("/api/chat/send", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(viewerToken.value ? { authorization: `Bearer ${viewerToken.value}` } : {})
      },
      body: JSON.stringify({ text })
    });
    if (!res.ok) {
      return;
    }
    chatText.value = "";
  } finally {
    chatSending.value = false;
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
    const res = await fetch("/api/viewer/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        username: viewerUsername.value,
        password: viewerPassword.value,
        turnstileToken: viewerTurnstileToken.value
      })
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data?.error || "登录失败");
    }
    viewerToken.value = data.token;
    viewerIdentity.value = data.username || viewerUsername.value.trim();
    localStorage.setItem("viewer_token", viewerToken.value);
    localStorage.setItem("viewer_identity", viewerIdentity.value);
    chatUser.value = viewerIdentity.value;
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
    const res = await fetch("/api/viewer/register/token", {
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
    const res = await fetch("/api/viewer/register/start", {
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
    viewerNotice.value = data?.devCode
      ? `验证码已发送（开发模式：${data.devCode}）`
      : "验证码已发送，请查收邮箱";
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
    const res = await fetch("/api/viewer/register/verify", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, username, password, code, turnstileToken: viewerTurnstileToken.value })
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data?.error || "注册失败");
    }
    viewerToken.value = data.token;
    viewerIdentity.value = data.username || username;
    localStorage.setItem("viewer_token", viewerToken.value);
    localStorage.setItem("viewer_identity", viewerIdentity.value);
    chatUser.value = viewerIdentity.value;
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
  if (!viewerToken.value) return;
  try {
    const res = await fetch("/api/viewer/me", {
      headers: { authorization: `Bearer ${viewerToken.value}` }
    });
    if (!res.ok) {
      viewerToken.value = "";
      viewerIdentity.value = "";
      localStorage.removeItem("viewer_token");
      localStorage.removeItem("viewer_identity");
      return;
    }
    const data = await res.json();
    viewerIdentity.value = data.username || viewerIdentity.value;
    chatUser.value = viewerIdentity.value || chatUser.value;
    if (viewerIdentity.value) showViewerAuth.value = false;
    await loadViewerProfile();
  } catch {
    // ignore
  }
};

const loadViewerProfile = async () => {
  if (!viewerToken.value) return;
  try {
    const res = await fetch("/api/viewer/profile", {
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
    const res = await fetch("/api/viewer/profile", {
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
  viewerToken.value = "";
  viewerIdentity.value = "";
  localStorage.removeItem("viewer_token");
  localStorage.removeItem("viewer_identity");
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

const startWhep = async (url) => {
  if (!url) return;
  playerError.value = "";
  playerStatus.value = "connecting";
  if (pc) {
    pc.close();
    pc = null;
  }
  if (statsTimer) {
    clearInterval(statsTimer);
    statsTimer = null;
  }

  pc = new RTCPeerConnection();
  pc.addTransceiver("video", { direction: "recvonly" });
  pc.addTransceiver("audio", { direction: "recvonly" });

  pc.ontrack = (event) => {
    const stream = event.streams[0];
    if (videoRef.value && stream) {
      videoRef.value.srcObject = stream;
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
    return;
  }

  if (!res.ok) {
    const text = await res.text();
    if (res.status === 404 && text.includes("no stream is available")) {
      playerStatus.value = "idle";
      playerError.value = "未开播";
      return;
    }
    playerStatus.value = "error";
    playerError.value = `WHEP 连接失败 ${res.status}: ${text || "Bad Request"}`;
    return;
  }

  const answerSdp = await res.text();
  if (!answerSdp) {
    playerStatus.value = "error";
    playerError.value = "WHEP 无返回 SDP";
    return;
  }
  await pc.setRemoteDescription({ type: "answer", sdp: answerSdp });
  playerStatus.value = "playing";
  statsTimer = setInterval(reportWhepStats, 5000);
};

onMounted(() => {
  loadAll();
  connectWs();
  loadViewerMe();
  ensureTurnstileScript();
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
});

onBeforeUnmount(() => {
  if (reconnectTimer) clearTimeout(reconnectTimer);
  if (refreshTimer) clearInterval(refreshTimer);
  if (ws) ws.close();
  if (pc) pc.close();
  if (statsTimer) clearInterval(statsTimer);
  if (resizeObserver) resizeObserver.disconnect();
  if (collapseMedia && collapseListener) {
    collapseMedia.removeEventListener("change", collapseListener);
  }
});

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
      callback: "onViewerTurnstile"
    });
  }
  if (turnstileRegisterRef.value) {
    turnstileRegisterId = window.turnstile.render(turnstileRegisterRef.value, {
      sitekey: "0x4AAAAAACncUgjk6YpyY6aB",
      callback: "onViewerTurnstile"
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
            <button class="meow-pill motion-press" :class="overlayTab === 'tasks' ? 'info-tab-active' : ''" type="button" @click="overlayTab = 'tasks'">任务</button>
            <button class="meow-pill motion-press" :class="overlayTab === 'ops' ? 'info-tab-active' : ''" type="button" @click="overlayTab = 'ops'">运营</button>
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
            </div>
            <div class="flex items-center gap-2">
              <span class="status-lamp" :class="stream.status === 'live' ? 'status-live' : (stream.status === 'ready' ? 'status-warn' : 'status-idle')"></span>
              <span class="text-sm">{{ stream.status === 'live' ? '直播中' : (stream.status === 'ready' ? '准备中' : '离线') }}</span>
            </div>
          </div>
          <div class="video-frame">
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
              <video class="viewer-video" controls autoplay muted playsinline :src="playInfo.hls"></video>
            </template>
            <div v-else class="viewer-screen-inner">
              <div class="viewer-screen-badge">Live</div>
              <div class="viewer-screen-title">等待播放器地址</div>
              <div class="viewer-screen-meta">请在后端配置播放地址</div>
            </div>
          </div>
          </div>
          <div v-if="playerError && !streamUnavailable" class="player-error mt-2 text-xs text-[#e06b8b]">{{ playerError }}</div>
          <div class="metrics-bar">
            <span>在线观众 {{ stream.viewers }}</span>
            <span class="metric-pill" :class="`metric-${bitrateStatus()}`">码率 {{ stream.bitrate }}</span>
            <span class="metric-pill" :class="`metric-${latencyStatus()}`">端到端 {{ stream.latency }}</span>
            <span class="metric-pill">推流延迟 {{ stream.pushLatency || "-" }}</span>
            <span class="metric-pill">播放延迟 {{ stream.playoutDelay || "-" }}</span>
            <span class="metric-pill">弹幕节奏 {{ chatRateLocal }}</span>
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
          实时弹幕节奏 {{ chatRateLocal }}
        </div>
        <div ref="chatScrollRef" class="chat-list mt-3" @scroll="onChatScroll">
          <transition-group ref="chatListRef" name="chat-slide" tag="div">
            <div v-for="item in displayedMessages" :key="item.id || `${item.user}-${item.text}`" class="chat-item">
              <button class="chat-user" type="button" @click="addMention(item.user)">{{ item.user }}</button>
              <span class="chat-text" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'" v-html="highlightMentions(item.text)"></span>
            </div>
          </transition-group>
        </div>
        <button v-if="chatHasNew" class="chat-new-tip" type="button" @click="scrollChatToBottom">
          下方有新消息
        </button>
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
