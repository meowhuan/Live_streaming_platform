<script setup>
import { ref, computed, onMounted, onBeforeUnmount, nextTick, watch } from "vue";

const isNight = ref(false);
const loading = ref(true);
const error = ref("");
const wsStatus = ref("disconnected");
const wsError = ref("");
const lastUpdated = ref("");
const showKey = ref(false);
const copyNotice = ref("");
const autoRefresh = ref(true);
const eventLog = ref([]);
const history = ref({
  viewers: [],
  bitrate: [],
  latency: []
});
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

const adminToken = ref(loadStoredToken("live_admin_token", "live_admin_token_expires_at", "live_admin_identity"));
const adminViewerToken = ref(loadStoredToken("viewer_token", "viewer_token_expires_at", "viewer_identity"));
const adminUser = ref(localStorage.getItem("live_admin_user") || "");
const adminPass = ref(localStorage.getItem("live_admin_pass") || "");
const adminRememberUser = ref(!!localStorage.getItem("live_admin_user"));
const adminRememberPass = ref(!!localStorage.getItem("live_admin_pass"));
const adminRememberDays = ref(Number(localStorage.getItem("live_admin_remember_days") || 0) || 0);
const adminIdentity = ref(localStorage.getItem("live_admin_identity") || "");
const adminSaving = ref(false);
const actionNotice = ref("");
const adminSessionOk = ref(false);
const isAuthed = computed(() => !!adminToken.value || adminSessionOk.value);
const smtpConfig = ref({
  host: "",
  port: 587,
  username: "",
  password: "",
  from: "",
  replyTo: "",
  starttls: true,
  passwordSet: false
});
const smtpSaving = ref(false);
const smtpNotice = ref("");
const smtpTestTo = ref("");
const smtpTestSending = ref(false);
const telegramConfig = ref({
  channel: "",
  tokenConfigured: false
});
const telegramSaving = ref(false);
const telegramNotice = ref("");
const notifyTemplates = ref({
  live: { title: "", message: "", url: "" },
  offline: { title: "", message: "", url: "" },
  schedule: { title: "", message: "", url: "" },
  live_url: "",
  rules: []
});
const notifyRulesRaw = ref("[]");
const notifySaving = ref(false);
const notifyNotice = ref("");
const antiAbuseConfig = ref({
  verifyEmailRateLimitWindowSecs: 1800,
  verifyEmailRateLimitMax: 3,
  verifyEmailRateLimitEmailMax: 2,
  verifyEmailCooldownSecs: 600,
  blockDisposableEmail: true,
  blockEduGovEmail: true,
  registerTokenTtlSecs: 600
});
const antiAbuseSaving = ref(false);
const antiAbuseNotice = ref("");
const form = ref({
  roomId: "",
  title: "",
  host: "",
  resolution: "",
  bitrate: "",
  latency: ""
});

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
  giftResponse: "-",
  giftLatency: "-",
  viewers: 0,
  updatedAt: ""
});

const streamStats = ref([]);
const healthChecks = ref([]);
const scheduleList = ref([]);
const scheduleDraft = ref([]);
const scheduleDirty = ref(false);
const scheduleSaving = ref(false);
const scheduleNotice = ref("");
const channelCards = ref([]);
const opsAlerts = ref([]);
const ingestNodes = ref([]);
const ingestConfig = ref([]);
const ingestInfo = ref({ srt: "", token: "" });
const playInfo = ref({ whep: "", hls: "" });
const ingestDisplay = ref([]);
const metricsInfo = ref({});


const toggleTheme = () => {
  isNight.value = !isNight.value;
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

const clearAdminSession = (notice) => {
  adminToken.value = "";
  localStorage.removeItem("live_admin_token");
  localStorage.removeItem("live_admin_token_expires_at");
  adminIdentity.value = "";
  localStorage.removeItem("live_admin_identity");
  adminSessionOk.value = false;
  if (ws) {
    ws.close();
    ws = null;
  }
  wsStatus.value = "disconnected";
  if (reconnectTimer) clearTimeout(reconnectTimer);
  if (refreshTimer) {
    clearInterval(refreshTimer);
    refreshTimer = null;
  }
  if (notice) {
    actionNotice.value = notice;
    setTimeout(() => {
      actionNotice.value = "";
    }, 2000);
  }
};

const loadAll = async () => {
  loading.value = true;
  error.value = "";
  try {
    const [
      streamRes,
      statsRes,
      healthRes,
      ingestRes,
      scheduleRes,
      channelsRes,
      alertsRes,
      nodesRes,
      ingestInfoRes,
      playInfoRes,
      metricsRes
    ] = await Promise.all([
      fetchJson("/api/stream"),
      fetchJson("/api/stream/stats"),
      fetchJson("/api/stream/health"),
      fetchJson("/api/ingest/config"),
      fetchJson("/api/schedule"),
      fetchJson("/api/channels"),
      fetchJson("/api/ops/alerts"),
      fetchJson("/api/ingest/nodes"),
      fetchJson("/api/stream/ingest"),
      fetchJson("/api/stream/play"),
      fetchJson("/api/mediamtx/metrics")
    ]);

    stream.value = streamRes;
    streamStats.value = statsRes.items || [];
    healthChecks.value = healthRes.items || [];
    ingestConfig.value = ingestRes.items || [];
    scheduleList.value = scheduleRes.items || [];
    if (!scheduleDirty.value) {
      scheduleDraft.value = normalizeSchedule(scheduleList.value);
    }
    channelCards.value = channelsRes.items || [];
    opsAlerts.value = alertsRes.items || [];
    ingestNodes.value = nodesRes.items || [];
    ingestInfo.value = ingestInfoRes || { srt: "", token: "" };
    playInfo.value = playInfoRes || { whep: "", hls: "" };
    metricsInfo.value = metricsRes.items || {};
    ingestDisplay.value = [
      ...(ingestInfo.value.srt
        ? [{
            protocol: "SRT",
            url: ingestInfo.value.srt,
            note: "后端签发推流地址",
            tag: "推流"
          }]
        : []),
      ...(playInfo.value.whep
        ? [{
            protocol: "WHEP",
            url: playInfo.value.whep,
            note: "观众端 WebRTC",
            tag: "播放"
          }]
        : []),
      ...(playInfo.value.hls
        ? [{
            protocol: "HLS",
            url: playInfo.value.hls,
            note: "观众端 HLS",
            tag: "播放"
          }]
        : [])
    ];
    lastUpdated.value = new Date().toLocaleTimeString("zh-CN");
    form.value = {
      roomId: streamRes.roomId || "",
      title: streamRes.title || "",
      host: streamRes.host || "",
      resolution: streamRes.resolution || "",
      bitrate: streamRes.bitrate || "",
      latency: streamRes.latency || ""
    };
    pushHistory(stream.value);
    pushEvent("数据刷新", "已从 API 拉取最新数据");
  } catch (err) {
    error.value = err instanceof Error ? err.message : "加载失败";
    pushEvent("数据刷新", "拉取失败");
  } finally {
    loading.value = false;
  }
};

const applySnapshot = (snapshot) => {
  if (!snapshot) return;
  if (snapshot.stream) stream.value = snapshot.stream;
  if (snapshot.stats) streamStats.value = snapshot.stats;
  if (snapshot.health) healthChecks.value = snapshot.health;
  if (snapshot.ingestConfig) ingestConfig.value = snapshot.ingestConfig;
  if (snapshot.schedule) scheduleList.value = snapshot.schedule;
  if (snapshot.channels) channelCards.value = snapshot.channels;
  if (snapshot.opsAlerts) opsAlerts.value = snapshot.opsAlerts;
  if (snapshot.nodes) ingestNodes.value = snapshot.nodes;
  if (snapshot.stream) pushHistory(snapshot.stream);
};

const applyUpdate = (type, data) => {
  switch (type) {
    case "stream:update":
      stream.value = data;
      lastUpdated.value = data?.updatedAt
        ? new Date(data.updatedAt).toLocaleTimeString("zh-CN")
        : new Date().toLocaleTimeString("zh-CN");
      pushHistory(data);
      pushEvent("推流更新", `状态 ${data?.status || "-"}`);
      break;
    case "stats:update":
      streamStats.value = data;
      pushEvent("统计更新", "控制台指标已更新");
      break;
    case "health:update":
      healthChecks.value = data;
      pushEvent("健康度", "推流健康度更新");
      break;
    case "ingest:update":
      ingestConfig.value = data;
      pushEvent("配置更新", "推流配置变更");
      break;
    case "schedule:update":
      scheduleList.value = data;
      if (!scheduleDirty.value) {
        scheduleDraft.value = normalizeSchedule(data);
      }
      pushEvent("排期更新", "排期已同步");
      break;
    case "channels:update":
      channelCards.value = data;
      pushEvent("频道更新", "频道列表已更新");
      break;
    case "ops:update":
      opsAlerts.value = data;
      pushEvent("运营提醒", "提醒内容更新");
      break;
    case "nodes:update":
      ingestNodes.value = data;
      pushEvent("节点更新", "推荐节点更新");
      break;
    default:
      break;
  }
};

const pushEvent = (title, detail) => {
  const entry = {
    id: `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`,
    title,
    detail,
    time: new Date().toLocaleTimeString("zh-CN")
  };
  eventLog.value = [entry, ...eventLog.value].slice(0, 12);
};

const clearEvents = () => {
  eventLog.value = [];
};

const parseNumber = (value) => {
  if (typeof value === "number") return value;
  if (!value) return null;
  const parsed = Number.parseFloat(String(value).replace(/[^0-9.]+/g, ""));
  return Number.isNaN(parsed) ? null : parsed;
};

const parseBitrateMbps = (value) => {
  if (!value) return null;
  const num = parseNumber(value);
  if (num === null) return null;
  const lower = String(value).toLowerCase();
  if (lower.includes("kbps")) return num / 1000;
  return num;
};

const parseLatencyMs = (value) => {
  if (!value) return null;
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

const pushHistory = (payload) => {
  if (!payload) return;
  const next = { ...history.value };
  const viewers = parseNumber(payload.viewers);
  const bitrate = parseNumber(payload.bitrate);
  const latency = parseNumber(payload.latency);

  if (viewers !== null) next.viewers = [...next.viewers, viewers].slice(-30);
  if (bitrate !== null) next.bitrate = [...next.bitrate, bitrate].slice(-30);
  if (latency !== null) next.latency = [...next.latency, latency].slice(-30);

  history.value = next;
};

const sparklinePoints = (values) => {
  if (!values || values.length < 2) return "";
  const max = Math.max(...values);
  const min = Math.min(...values);
  const range = max - min || 1;
  return values
    .map((value, index) => {
      const x = (index / (values.length - 1)) * 100;
      const y = 100 - ((value - min) / range) * 100;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
};

const trendValue = (values) => {
  if (!values || values.length < 2) return { diff: 0, up: true };
  const diff = values[values.length - 1] - values[0];
  return { diff, up: diff >= 0 };
};

const formatTime = (value) => {
  if (!value) return "-";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "-";
  return date.toLocaleTimeString("zh-CN");
};

const isKeyItem = (item) => item?.label === "Key";

const getDisplayValue = (item) => {
  const raw = item?.url || item?.value || "-";
  if (!isKeyItem(item)) return raw;
  return showKey.value ? raw : "••••••••••••";
};

const copyText = async (text) => {
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    copyNotice.value = "已复制";
  } catch {
    copyNotice.value = "复制失败";
  } finally {
    setTimeout(() => {
      copyNotice.value = "";
    }, 1600);
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

let ws = null;
let reconnectTimer = null;

const adminFetch = async (url, payload) => {
  adminSaving.value = true;
  actionNotice.value = "";
  try {
    const hasBody = payload !== undefined && payload !== null;
    const res = await fetch(apiUrl(url), {
      method: "POST",
      credentials: "include",
      headers: {
        ...(hasBody ? { "Content-Type": "application/json" } : {}),
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      },
      body: hasBody ? JSON.stringify(payload) : undefined
    });
      if (res.status === 401) {
      clearAdminSession("登录已失效，请重新登录");
      return false;
    }
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || `请求失败 ${res.status}`);
    }
    adminSessionOk.value = true;
    actionNotice.value = "操作成功";
    loadAll();
    return true;
  } catch (err) {
    actionNotice.value = err instanceof Error ? err.message : "操作失败";
    return false;
  } finally {
    adminSaving.value = false;
    setTimeout(() => {
      actionNotice.value = "";
    }, 2000);
  }
};

const loginAdmin = async () => {
  if (!adminUser.value || !adminPass.value) {
    actionNotice.value = "请输入账号与密码";
    return;
  }
  adminSaving.value = true;
  actionNotice.value = "";
  try {
    const res = await fetch(apiUrl("/api/admin/login"), {
      method: "POST",
      credentials: "include",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        username: adminUser.value,
        password: adminPass.value,
        rememberDays: adminRememberDays.value || undefined
      })
    });
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || "登录失败");
    }
    const data = await res.json();
    adminToken.value = data.token;
    localStorage.setItem("live_admin_token", data.token);
    if (data.expiresIn) {
      localStorage.setItem("live_admin_token_expires_at", String(Date.now() + Number(data.expiresIn) * 1000));
    }
    adminIdentity.value = adminUser.value.trim();
    localStorage.setItem("live_admin_identity", adminIdentity.value);
    if (data.viewerToken) {
      localStorage.setItem("viewer_token", data.viewerToken);
      localStorage.setItem("viewer_identity", data.viewerName || adminIdentity.value);
      adminViewerToken.value = data.viewerToken;
      if (data.expiresIn) {
        localStorage.setItem("viewer_token_expires_at", String(Date.now() + Number(data.expiresIn) * 1000));
      }
    }
    if (adminRememberUser.value) {
      localStorage.setItem("live_admin_user", adminUser.value.trim());
    } else {
      localStorage.removeItem("live_admin_user");
    }
    if (adminRememberPass.value) {
      localStorage.setItem("live_admin_pass", adminPass.value);
    } else {
      localStorage.removeItem("live_admin_pass");
    }
    if (adminRememberDays.value) {
      localStorage.setItem("live_admin_remember_days", String(adminRememberDays.value));
    } else {
      localStorage.removeItem("live_admin_remember_days");
    }
    adminPass.value = "";
    adminSessionOk.value = true;
    actionNotice.value = "已登录";
    enterAdmin();
  } catch (err) {
    actionNotice.value = err instanceof Error ? err.message : "登录失败";
  } finally {
    adminSaving.value = false;
    setTimeout(() => {
      actionNotice.value = "";
    }, 2000);
  }
};

const logoutAdmin = async () => {
  try {
      await fetch(apiUrl("/api/admin/logout"), { method: "POST", credentials: "include" });
  } catch {
    // ignore
  }
  clearAdminSession("已退出");
};

const loadSmtp = async () => {
  try {
    const res = await fetch(apiUrl("/api/admin/smtp"), {
      credentials: "include",
      headers: {
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      }
    });
    if (res.status === 401) {
      clearAdminSession("登录已失效，请重新登录");
      return;
    }
    if (!res.ok) return;
    const data = await res.json();
    if (data?.data) {
      smtpConfig.value = {
        host: data.data.host || "",
        port: data.data.port || 587,
        username: data.data.username || "",
        password: "",
        from: data.data.from || "",
        replyTo: data.data.reply_to || "",
        starttls: data.data.starttls ?? true,
        passwordSet: !!data.data.password_set
      };
    }
  } catch {
    // ignore
  }
};

const loadAdminSession = async () => {
  try {
    const res = await fetch(apiUrl("/api/admin/me"), {
      credentials: "include",
      headers: {
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      }
    });
    if (!res.ok) return;
    adminSessionOk.value = true;
    if (!adminIdentity.value) adminIdentity.value = "已登录";
    enterAdmin();
  } catch {
    // ignore
  }
};

const loadTelegram = async () => {
  try {
    const res = await fetch(apiUrl("/api/admin/telegram/channel"), {
      credentials: "include",
      headers: {
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      }
    });
    if (res.status === 401) {
      clearAdminSession("登录已失效，请重新登录");
      return;
    }
    if (!res.ok) return;
    const data = await res.json();
    telegramConfig.value = {
      channel: data.channel || "",
      tokenConfigured: !!data.token_configured
    };
  } catch {
    // ignore
  }
};

const loadNotifyTemplates = async () => {
  try {
    const res = await fetch(apiUrl("/api/admin/notify-templates"), {
      credentials: "include",
      headers: {
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      }
    });
    if (res.status === 401) {
      clearAdminSession("登录已失效，请重新登录");
      return;
    }
    if (!res.ok) return;
    const data = await res.json();
    if (data?.data) {
      notifyTemplates.value = {
        live: data.data.live || notifyTemplates.value.live,
        offline: data.data.offline || notifyTemplates.value.offline,
        schedule: data.data.schedule || notifyTemplates.value.schedule,
        live_url: data.data.live_url || "",
        rules: data.data.rules || []
      };
      notifyRulesRaw.value = JSON.stringify(notifyTemplates.value.rules || [], null, 2);
    }
  } catch {
    // ignore
  }
};

const saveNotifyTemplates = async () => {
  notifySaving.value = true;
  notifyNotice.value = "";
  try {
    let rules = [];
    if (notifyRulesRaw.value.trim()) {
      rules = JSON.parse(notifyRulesRaw.value);
      if (!Array.isArray(rules)) throw new Error("规则必须是数组 JSON");
    }
    const payload = {
      live: notifyTemplates.value.live,
      offline: notifyTemplates.value.offline,
      schedule: notifyTemplates.value.schedule,
      live_url: notifyTemplates.value.live_url || "",
      rules
    };
    const res = await fetch(apiUrl("/api/admin/notify-templates"), {
      method: "POST",
      credentials: "include",
      headers: {
        "Content-Type": "application/json",
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      },
      body: JSON.stringify(payload)
    });
    if (res.status === 401) {
      clearAdminSession("登录已失效，请重新登录");
      return;
    }
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || "保存失败");
    }
    notifyNotice.value = "通知模板已保存";
    loadNotifyTemplates();
  } catch (err) {
    notifyNotice.value = err instanceof Error ? err.message : "保存失败";
  } finally {
    notifySaving.value = false;
    setTimeout(() => {
      notifyNotice.value = "";
    }, 2000);
  }
};

const saveTelegram = async () => {
  telegramSaving.value = true;
  telegramNotice.value = "";
  try {
    const res = await fetch(apiUrl("/api/admin/telegram/channel"), {
      method: "POST",
      credentials: "include",
      headers: {
        "Content-Type": "application/json",
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      },
      body: JSON.stringify({
        channel: telegramConfig.value.channel
      })
    });
    if (res.status === 401) {
      clearAdminSession("登录已失效，请重新登录");
      return;
    }
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || "保存失败");
    }
    telegramNotice.value = "Telegram 频道已保存";
    loadTelegram();
  } catch (err) {
    telegramNotice.value = err instanceof Error ? err.message : "保存失败";
  } finally {
    telegramSaving.value = false;
    setTimeout(() => {
      telegramNotice.value = "";
    }, 2000);
  }
};


const loadViewerAntiAbuse = async () => {
  try {
    const res = await fetch(apiUrl("/api/admin/viewer-anti-abuse"), {
      credentials: "include",
      headers: {
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      }
    });
    if (res.status === 401) {
      clearAdminSession("登录已失效，请重新登录");
      return;
    }
    if (!res.ok) return;
    const data = await res.json();
    antiAbuseConfig.value = {
      verifyEmailRateLimitWindowSecs: data.verify_email_rate_limit_window_secs ?? 1800,
      verifyEmailRateLimitMax: data.verify_email_rate_limit_max ?? 3,
      verifyEmailRateLimitEmailMax: data.verify_email_rate_limit_email_max ?? 2,
      verifyEmailCooldownSecs: data.verify_email_cooldown_secs ?? 600,
      blockDisposableEmail: data.block_disposable_email ?? true,
      blockEduGovEmail: data.block_edu_gov_email ?? true,
      registerTokenTtlSecs: data.register_token_ttl_secs ?? 600
    };
  } catch {
    // ignore
  }
};

const saveViewerAntiAbuse = async () => {
  antiAbuseSaving.value = true;
  antiAbuseNotice.value = "";
  try {
    const payload = {
      verify_email_rate_limit_window_secs: Number(antiAbuseConfig.value.verifyEmailRateLimitWindowSecs) || 1800,
      verify_email_rate_limit_max: Number(antiAbuseConfig.value.verifyEmailRateLimitMax) || 3,
      verify_email_rate_limit_email_max: Number(antiAbuseConfig.value.verifyEmailRateLimitEmailMax) || 2,
      verify_email_cooldown_secs: Number(antiAbuseConfig.value.verifyEmailCooldownSecs) || 600,
      block_disposable_email: !!antiAbuseConfig.value.blockDisposableEmail,
      block_edu_gov_email: !!antiAbuseConfig.value.blockEduGovEmail,
      register_token_ttl_secs: Number(antiAbuseConfig.value.registerTokenTtlSecs) || 600
    };
    const res = await fetch(apiUrl("/api/admin/viewer-anti-abuse"), {
      method: "POST",
      credentials: "include",
      headers: {
        "Content-Type": "application/json",
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      },
      body: JSON.stringify(payload)
    });
    if (res.status === 401) {
      clearAdminSession("登录已失效，请重新登录");
      return;
    }
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || "保存失败");
    }
    antiAbuseNotice.value = "防盗刷配置已保存";
  } catch (err) {
    antiAbuseNotice.value = err instanceof Error ? err.message : "保存失败";
  } finally {
    antiAbuseSaving.value = false;
    setTimeout(() => {
      antiAbuseNotice.value = "";
    }, 2000);
  }
};

const saveSmtp = async () => {
  smtpSaving.value = true;
  smtpNotice.value = "";
  try {
    const payload = {
      host: smtpConfig.value.host,
      port: Number(smtpConfig.value.port) || 587,
      username: smtpConfig.value.username,
      password: smtpConfig.value.password,
      from: smtpConfig.value.from,
      reply_to: smtpConfig.value.replyTo || null,
      starttls: smtpConfig.value.starttls
    };
    const res = await fetch(apiUrl("/api/admin/smtp"), {
      method: "POST",
      credentials: "include",
      headers: {
        "Content-Type": "application/json",
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      },
      body: JSON.stringify(payload)
    });
    if (res.status === 401) {
      clearAdminSession("登录已失效，请重新登录");
      return;
    }
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || "保存失败");
    }
    smtpConfig.value.password = "";
    smtpConfig.value.passwordSet = true;
    smtpNotice.value = "SMTP 已保存";
  } catch (err) {
    smtpNotice.value = err instanceof Error ? err.message : "保存失败";
  } finally {
    smtpSaving.value = false;
    setTimeout(() => {
      smtpNotice.value = "";
    }, 2000);
  }
};

const sendSmtpTest = async () => {
  if (!smtpTestTo.value.trim()) {
    smtpNotice.value = "请输入测试邮箱";
    return;
  }
  smtpTestSending.value = true;
  smtpNotice.value = "";
  try {
    const res = await fetch(apiUrl("/api/admin/smtp/test"), {
      method: "POST",
      credentials: "include",
      headers: {
        "Content-Type": "application/json",
        ...(adminToken.value ? { authorization: `Bearer ${adminToken.value}` } : {})
      },
      body: JSON.stringify({ to: smtpTestTo.value.trim() })
    });
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || "发送失败");
    }
    smtpNotice.value = "测试邮件已发送";
  } catch (err) {
    smtpNotice.value = err instanceof Error ? err.message : "发送失败";
  } finally {
    smtpTestSending.value = false;
    setTimeout(() => {
      smtpNotice.value = "";
    }, 2000);
  }
};

const startStream = () => adminFetch("/api/admin/stream/update", {
  status: "live",
  roomId: form.value.roomId,
  title: form.value.title,
  host: form.value.host,
  resolution: form.value.resolution,
  bitrate: form.value.bitrate,
  latency: form.value.latency
});

const stopStream = () => adminFetch("/api/admin/stream/update", {
  status: "offline"
});

const updateStreamInfo = () => adminFetch("/api/admin/stream/update", {
  roomId: form.value.roomId,
  title: form.value.title,
  host: form.value.host,
  resolution: form.value.resolution,
  bitrate: form.value.bitrate,
  latency: form.value.latency
});

const createRoom = async () => adminFetch("/api/admin/room/create");
const refreshIngestToken = async () => adminFetch("/api/admin/ingest/refresh");

let refreshTimer = null;

const enterAdmin = () => {
  loadAll();
  loadSmtp();
  loadTelegram();
  loadNotifyTemplates();
  loadViewerAntiAbuse();
  connectWs();
  if (!refreshTimer) {
    refreshTimer = setInterval(() => {
      if (autoRefresh.value && wsStatus.value !== "connected") {
        loadAll();
      }
    }, 20000);
  }
};

const connectWs = () => {
  if (ws) {
    ws.close();
    ws = null;
  }
  wsStatus.value = "connecting";
  wsError.value = "";

  ws = new WebSocket(wsUrl());
  ws.onopen = () => {
    wsStatus.value = "connected";
    pushEvent("WS 连接", "实时通道已连接");
  };
  ws.onmessage = (event) => {
    try {
      const payload = JSON.parse(event.data);
      if (payload.type === "snapshot") {
        applySnapshot(payload.data);
      } else if (payload.type && payload.data) {
        applyUpdate(payload.type, payload.data);
      }
    } catch {
      wsError.value = "WS 数据解析失败";
      pushEvent("WS 解析", "消息解析失败");
    }
  };
  ws.onerror = () => {
    wsError.value = "WS 连接错误";
    pushEvent("WS 错误", "连接出现错误");
  };
  ws.onclose = () => {
    wsStatus.value = "disconnected";
    pushEvent("WS 断开", "正在重连");
    if (reconnectTimer) clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(() => {
      connectWs();
    }, 3000);
  };
};

  onMounted(() => {
  watch(() => !isAuthed.value, () => {}, { immediate: true });
    if (adminToken.value) {
      enterAdmin();
    } else {
      loadAdminSession().finally(() => {
        if (!isAuthed.value) loading.value = false;
    });
  }
});


const normalizeSchedule = (items) => {
  if (!Array.isArray(items)) return [];
  return items.map((item) => ({
    time: item?.time || "",
    title: item?.title || "",
    tag: item?.tag || "",
    host: item?.host || ""
  }));
};

const addScheduleItem = () => {
  scheduleDraft.value = [
    ...scheduleDraft.value,
    { time: "20:00", title: "夜猫直播间", tag: "常规", host: stream.value.host || "Meowhuan" }
  ];
  scheduleDirty.value = true;
};

const removeScheduleItem = (index) => {
  scheduleDraft.value = scheduleDraft.value.filter((_, idx) => idx !== index);
  scheduleDirty.value = true;
};

const resetScheduleDraft = () => {
  scheduleDraft.value = normalizeSchedule(scheduleList.value);
  scheduleDirty.value = false;
};

const saveSchedule = async () => {
  scheduleSaving.value = true;
  scheduleNotice.value = "";
  const payload = scheduleDraft.value.map((item) => ({
    time: item.time?.trim(),
    title: item.title?.trim(),
    tag: item.tag?.trim(),
    host: item.host?.trim()
  })).filter((item) => item.time || item.title);
  const ok = await adminFetch("/api/admin/schedule/replace", payload);
  scheduleSaving.value = false;
  if (ok) {
    scheduleDirty.value = false;
    scheduleNotice.value = "已保存";
  } else {
    scheduleNotice.value = scheduleNotice.value || "保存失败";
  }
  setTimeout(() => {
    scheduleNotice.value = "";
  }, 2000);
};

onBeforeUnmount(() => {
  if (reconnectTimer) clearTimeout(reconnectTimer);
  if (refreshTimer) clearInterval(refreshTimer);
  if (ws) ws.close();
});
</script>

<template>
  <div
    class="min-h-screen font-body page-fade transition-colors duration-700 ease-in-out meow-bg"
    :class="isNight
      ? 'bg-gradient-to-br from-meow-night-bg via-[#201a3f] to-[#16162a] text-meow-night-ink meow-night'
      : 'bg-gradient-to-br from-meow-bg via-[#fff6fb] to-[#f2f0ff] text-meow-ink meow-day'"
  >
    <div class="relative overflow-hidden">
      <div
        class="pointer-events-none absolute -left-32 -top-24 h-80 w-80 rounded-[45%_55%_60%_40%/50%_60%_40%_50%] blur-3xl opacity-70 animate-floaty"
        :class="isNight
          ? 'bg-[radial-gradient(circle_at_top,_#3a2b6f,_transparent_65%)]'
          : 'bg-[radial-gradient(circle_at_top,_#ffd4e6,_transparent_65%)]'"
      ></div>
        <div
          class="pointer-events-none absolute -right-32 top-24 h-96 w-96 rounded-[55%_45%_45%_55%/45%_55%_45%_55%] blur-3xl opacity-70 animate-floaty"
          :class="isNight
            ? 'bg-[radial-gradient(circle_at_top,_#1d5c7a,_transparent_65%)]'
            : 'bg-[radial-gradient(circle_at_top,_#c8f6ed,_transparent_65%)]'"
        ></div>

        <div v-if="!isAuthed" class="auth-overlay">
          <div class="auth-card" :class="isNight ? 'auth-card-night' : 'auth-card-day'">
            <div class="flex items-center justify-between">
              <div>
                <div class="text-xs uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">Admin Access</div>
                <div class="font-display text-2xl mt-1">进入管理后台</div>
              </div>
            </div>

            <div class="mt-6 grid gap-4">
              <div class="field-group">
                <label class="field-label">管理员账号 / 邮箱</label>
                <input v-model="adminUser" class="field-input" placeholder="admin 或 meow@example.com" />
              </div>
              <div class="field-group">
                <label class="field-label">管理员密码</label>
                <input v-model="adminPass" class="field-input" type="password" placeholder="••••••••" />
              </div>
              <div class="field-group">
                <label class="field-label">登录保持</label>
                <div class="flex flex-wrap items-center gap-3 text-xs">
                  <label class="meow-pill motion-press">
                    <input type="checkbox" class="mr-2" v-model="adminRememberUser" />
                    记住账号
                  </label>
                  <label class="meow-pill motion-press">
                    <input type="checkbox" class="mr-2" v-model="adminRememberPass" />
                    记住密码（仅本机）
                  </label>
                  <select v-model="adminRememberDays" class="field-input !h-8 !text-xs w-[140px]">
                    <option :value="0">默认时长</option>
                    <option :value="7">保持 7 天</option>
                    <option :value="30">保持 30 天</option>
                  </select>
                </div>
              </div>
              <div class="flex flex-wrap gap-2">
                <button class="meow-btn-primary motion-press" type="button" :disabled="adminSaving" @click="loginAdmin">
                  登录
                </button>
              </div>
              <div v-if="actionNotice" class="text-xs text-[#e06b8b]">{{ actionNotice }}</div>
            </div>

          </div>
        </div>

        <div v-if="isAuthed" class="mx-auto w-[min(1100px,92vw)] pb-20 pt-8 relative">
        <button
          class="cord-switch cord-switch-mobile md:hidden"
          type="button"
          @click="toggleTheme"
          :class="isNight ? 'cord-switch-night' : 'cord-switch-day'"
          aria-label="切换深夜模式"
        >
          <span class="cord-line"></span>
          <span class="cord-knob">{{ isNight ? "🌙" : "☀️" }}</span>
          <span class="cord-label" aria-hidden="true"></span>
        </button>

        <nav class="flex items-center justify-between gap-4">
          <div class="flex items-center gap-3">
            <img
              src="/logo.png"
              alt="Meowhuan logo"
              class="h-10 w-10 rounded-full border bg-white/70 object-cover shadow-sm"
              :class="isNight ? 'border-meow-night-line' : 'border-meow-line'"
            />
            <div class="font-display text-xl tracking-wide">喵喵推流平台</div>
          </div>
          <div class="nav-links-wrap hidden items-center justify-end gap-5 text-sm md:flex" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
            <button
              class="cord-switch cord-switch-desktop cord-switch-desktop-left"
              type="button"
              @click="toggleTheme"
              :class="isNight ? 'cord-switch-night' : 'cord-switch-day'"
              aria-label="切换深夜模式"
            >
              <span class="cord-line"></span>
              <span class="cord-knob">{{ isNight ? "🌙" : "☀️" }}</span>
              <span class="cord-label" aria-hidden="true"></span>
            </button>
            <a class="nav-link" :class="isNight ? 'hover:text-meow-night-ink' : 'hover:text-meow-ink'" href="#dashboard">控制台</a>
            <a class="nav-link" :class="isNight ? 'hover:text-meow-night-ink' : 'hover:text-meow-ink'" href="#ingest">推流配置</a>
            <a class="nav-link" :class="isNight ? 'hover:text-meow-night-ink' : 'hover:text-meow-ink'" href="#smtp">SMTP</a>
            <a class="nav-link" :class="isNight ? 'hover:text-meow-night-ink' : 'hover:text-meow-ink'" href="#notify">通知</a>
            <a class="nav-link" :class="isNight ? 'hover:text-meow-night-ink' : 'hover:text-meow-ink'" href="#anti-abuse">防盗刷</a>
            <a class="nav-link" :class="isNight ? 'hover:text-meow-night-ink' : 'hover:text-meow-ink'" href="#schedule">排期</a>
            <a class="nav-link" :class="isNight ? 'hover:text-meow-night-ink' : 'hover:text-meow-ink'" href="#channels">频道</a>
            <a class="nav-link" :class="isNight ? 'hover:text-meow-night-ink' : 'hover:text-meow-ink'" href="#ops">运营</a>
            <button
              class="cord-switch cord-switch-desktop cord-switch-desktop-right"
              type="button"
              @click="toggleTheme"
              :class="isNight ? 'cord-switch-night' : 'cord-switch-day'"
              aria-label="切换深夜模式"
            >
              <span class="cord-line"></span>
              <span class="cord-knob">{{ isNight ? "🌙" : "☀️" }}</span>
              <span class="cord-label" aria-hidden="true"></span>
            </button>
          </div>
        </nav>

        <section class="mt-8">
          <div
            class="meow-card motion-card p-4 text-sm flex flex-wrap items-center justify-between gap-3"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex flex-wrap items-center gap-2">
              <span class="meow-pill">数据源</span>
              <span :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                {{ loading ? "同步中" : (error ? "同步失败" : (wsStatus === 'connected' ? "实时连接" : "已连接")) }}
              </span>
              <span class="status-pill" :class="wsStatus === 'connected' ? 'status-live' : 'status-idle'">
                <span class="status-dot"></span>
                {{ wsStatus === "connected" ? "WS 在线" : "WS 重连中" }}
              </span>
              <span class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                上次更新 {{ lastUpdated || "-" }}
              </span>
            </div>
            <button
              class="meow-btn-ghost motion-press"
              :class="isNight ? 'border-meow-night-line text-meow-night-ink hover:bg-meow-night-card/80' : ''"
              @click="loadAll"
            >
              {{ loading ? "刷新中" : "刷新数据" }}
            </button>
            <span v-if="error || wsError" class="text-xs text-[#e06b8b]">{{ error || wsError }}</span>
          </div>
        </section>

        <section class="mt-5">
          <div
            class="meow-card motion-card p-4"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex items-center justify-between">
              <div class="font-display text-base">连接设置</div>
              <span class="meow-pill">实时</span>
            </div>
            <div class="mt-3 space-y-3 text-sm">
              <div class="flex items-center justify-between gap-3">
                <span>自动刷新（断线时）</span>
                <button
                  class="toggle-btn"
                  type="button"
                  :class="autoRefresh ? 'toggle-on' : 'toggle-off'"
                  @click="autoRefresh = !autoRefresh"
                  aria-label="切换自动刷新"
                >
                  <span class="toggle-knob"></span>
                </button>
              </div>
              <div class="flex items-center justify-between gap-3">
                <span>最近更新</span>
                <span class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  {{ lastUpdated || "-" }}
                </span>
              </div>
              <div class="flex items-center justify-between gap-3">
                <span>WebSocket</span>
                <span class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  {{ wsStatus === "connected" ? "在线" : "重连中" }}
                </span>
              </div>
            </div>
          </div>
        </section>

        <section class="mt-6 grid gap-6 md:grid-cols-[1.1fr_0.9fr]">
          <div class="space-y-5">
            <span class="meow-pill">🎥 Live Studio</span>
            <h1 class="font-display text-4xl leading-tight sm:text-5xl">
              柔软、稳定的直播推流控制台。
            </h1>
            <p class="text-base leading-relaxed" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
              一个专注直播推流体验的面板：让主播能随时掌握链路状态、房间节奏、互动热度，
              并快速切换场景与节点，保持稳定、舒服的直播节奏。
            </p>
            <div class="flex flex-wrap gap-2">
              <span class="meow-pill">多平台转推</span>
              <span class="meow-pill">智能节点调度</span>
              <span class="meow-pill">弹幕健康监控</span>
            </div>
          </div>

          <div
            class="meow-card motion-card p-5 grid-glass"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex items-center justify-between">
              <div class="font-display text-lg">直播预览</div>
              <span class="stream-chip" :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''">
                <span class="stream-chip-dot"></span>
                {{ stream.status === 'live' ? 'On Air' : 'Offline' }}
              </span>
            </div>
            <div class="mt-4 stream-preview" :class="isNight ? 'stream-preview-night' : ''">
              <div class="stream-preview-content">
                <div class="flex items-center justify-between">
                  <div class="stream-badge" :class="isNight ? 'bg-meow-night-bg text-meow-night-ink border-meow-night-line' : ''">
                    房间 {{ stream.roomId }} · {{ stream.resolution }}
                  </div>
                  <div class="flex items-center gap-2 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                    <span
                      class="status-lamp"
                      :class="stream.status === 'live' ? 'status-live' : (stream.status === 'ready' ? 'status-warn' : 'status-idle')"
                    ></span>
                    {{ stream.status === 'live' ? '直播中' : (stream.status === 'ready' ? '准备中' : '离线') }}
                  </div>
                </div>
                <div>
                  <div class="text-lg font-600">{{ stream.title }}</div>
                  <div class="mt-2 flex items-center gap-2 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                    <span class="stream-avatar"></span>
                    {{ stream.host }}
                  </div>
                  <div class="mt-2 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                    观众 {{ stream.viewers }}
                    ·
                    <span class="metric-pill" :class="`metric-${bitrateStatus()}`">码率 {{ stream.bitrate }}</span>
                    ·
                    <span class="metric-pill" :class="`metric-${latencyStatus()}`">端到端 {{ stream.latency }}</span>
                    ·
                    <span class="metric-pill">推流延迟 {{ stream.pushLatency || "-" }}</span>
                    ·
                    <span class="metric-pill">播放延迟 {{ stream.playoutDelay || "-" }}</span>
                    ·
                    <span class="metric-pill">当前帧率 {{ stream.fps || "-" }}</span>
                  </div>
                </div>
              </div>
            </div>
            <div class="mt-4 grid gap-3 sm:grid-cols-3">
              <div class="stat-tile" :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''">
                <div class="text-[11px] uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  弹幕节奏
                </div>
                <div class="stat-value">{{ stream.chatRate }}</div>
                <div class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">热度上升</div>
              </div>
              <div class="stat-tile" :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''">
                <div class="text-[11px] uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  当前帧率
                </div>
                <div class="stat-value">{{ stream.fps }}</div>
                <div class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">播放实时统计</div>
              </div>
              <div class="stat-tile" :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''">
                <div class="text-[11px] uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  礼物响应
                </div>
                <div class="stat-value">{{ stream.giftResponse }}</div>
                <div class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">延迟 {{ stream.giftLatency }}</div>
              </div>
            </div>
          </div>
        </section>

        <section class="mt-8">
          <div
            class="meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex flex-wrap items-center justify-between gap-3">
              <h2 class="font-display text-xl">推流控制面板</h2>
              <span class="meow-pill">本地测试</span>
            </div>
            <div class="mt-4 grid gap-4 md:grid-cols-[1fr_1.4fr]">
              <div class="space-y-3">
                <label class="field-label">管理员会话</label>
                <div class="text-sm font-600">{{ adminIdentity || "已登录" }}</div>
                <div class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  状态：{{ isAuthed ? "已登录" : "未登录" }}
                </div>
                <div class="mt-2 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  ViewerToken：{{ adminViewerToken ? "已生成" : "未生成" }}
                </div>
                <div class="mt-2 flex flex-wrap items-center gap-2 text-[11px]">
                  <button class="copy-btn" type="button" @click="copyText(adminViewerToken)">
                    复制 ViewerToken
                  </button>
                </div>
                <div class="flex flex-wrap gap-2">
                  <button
                    class="meow-pill motion-press"
                    type="button"
                    :disabled="adminSaving"
                    @click="logoutAdmin"
                  >
                    退出登录
                  </button>
                </div>
                <div v-if="actionNotice" class="text-xs text-[#e06b8b]">{{ actionNotice }}</div>
              </div>
              <div class="grid gap-3 md:grid-cols-2">
                <div class="field-group">
                  <label class="field-label">房间号</label>
                  <input v-model="form.roomId" class="field-input" placeholder="1024" />
                </div>
                <div class="field-group">
                  <label class="field-label">频道标题</label>
                  <input v-model="form.title" class="field-input" placeholder="夜间陪伴频道" />
                </div>
                <div class="field-group">
                  <label class="field-label">主播</label>
                  <input v-model="form.host" class="field-input" placeholder="Meowhuan" />
                </div>
                <div class="field-group">
                  <label class="field-label">分辨率</label>
                  <input v-model="form.resolution" class="field-input" placeholder="1080p" />
                </div>
                <div class="field-group">
                  <label class="field-label">码率</label>
                  <input v-model="form.bitrate" class="field-input" placeholder="4.5 Mbps" />
                </div>
                <div class="field-group">
                  <label class="field-label">延迟</label>
                  <input v-model="form.latency" class="field-input" placeholder="120ms" />
                </div>
              </div>
            </div>
            <div class="mt-4 flex flex-wrap gap-3">
              <button
                class="meow-btn-ghost motion-press"
                :class="isNight ? 'border-meow-night-line text-meow-night-ink hover:bg-meow-night-card/80' : ''"
                :disabled="adminSaving"
                @click="createRoom"
              >
                创建直播间
              </button>
              <button
                class="meow-btn-primary motion-press"
                :class="isNight ? 'bg-meow-night-accent text-meow-night-bg' : ''"
                :disabled="adminSaving"
                @click="updateStreamInfo"
              >
                更新房间信息
              </button>
              <button
                class="meow-btn-ghost motion-press"
                :class="isNight ? 'border-meow-night-line text-meow-night-ink hover:bg-meow-night-card/80' : ''"
                :disabled="adminSaving"
                @click="startStream"
              >
                立即开播
              </button>
              <button
                class="meow-btn-ghost motion-press"
                :class="isNight ? 'border-meow-night-line text-meow-night-ink hover:bg-meow-night-card/80' : ''"
                :disabled="adminSaving"
                @click="stopStream"
              >
                结束推流
              </button>
            </div>
            <div class="mt-5">
              <div class="text-[11px] uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">MediaMTX 指标</div>
              <div class="mt-2 grid gap-2 md:grid-cols-2">
                <div
                  v-for="(value, key) in metricsInfo"
                  :key="key"
                  class="stat-tile"
                  :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''"
                >
                  <div class="text-xs break-all">{{ key }}</div>
                  <div class="text-sm font-600">{{ value }}</div>
                </div>
                <div v-if="Object.keys(metricsInfo).length === 0" class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  暂无指标
                </div>
              </div>
            </div>
          </div>
        </section>

        <section id="dashboard" class="mt-14">
          <div class="flex flex-wrap items-center justify-between gap-3">
            <h2 class="font-display text-2xl">直播控制台</h2>
            <span class="meow-pill meow-pill-strong">当前链路稳定</span>
          </div>
          <div class="mt-4 grid gap-4 md:grid-cols-3">
            <div class="meow-card motion-card p-4" :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''">
              <div class="flex items-center justify-between">
                <span class="text-xs uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">观众趋势</span>
                <span class="trend-pill" :class="trendValue(history.viewers).up ? 'trend-up' : 'trend-down'">
                  {{ trendValue(history.viewers).up ? "上升" : "下降" }}
                </span>
              </div>
              <div class="mt-2 text-lg font-600">{{ stream.viewers }}</div>
              <svg class="sparkline" viewBox="0 0 100 100" aria-hidden="true">
                <polyline :points="sparklinePoints(history.viewers)" class="sparkline-line" />
              </svg>
            </div>
            <div class="meow-card motion-card p-4" :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''">
              <div class="flex items-center justify-between">
                <span class="text-xs uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">码率趋势</span>
                <span class="trend-pill" :class="trendValue(history.bitrate).up ? 'trend-up' : 'trend-down'">
                  {{ trendValue(history.bitrate).up ? "上升" : "下降" }}
                </span>
              </div>
              <div class="mt-2 text-lg font-600">{{ stream.bitrate }}</div>
              <svg class="sparkline" viewBox="0 0 100 100" aria-hidden="true">
                <polyline :points="sparklinePoints(history.bitrate)" class="sparkline-line" />
              </svg>
            </div>
            <div class="meow-card motion-card p-4" :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''">
              <div class="flex items-center justify-between">
                <span class="text-xs uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">延迟趋势</span>
                <span class="trend-pill" :class="trendValue(history.latency).up ? 'trend-down' : 'trend-up'">
                  {{ trendValue(history.latency).up ? "上升" : "下降" }}
                </span>
              </div>
              <div class="mt-2 text-lg font-600">{{ stream.latency }}</div>
              <svg class="sparkline" viewBox="0 0 100 100" aria-hidden="true">
                <polyline :points="sparklinePoints(history.latency)" class="sparkline-line" />
              </svg>
            </div>
          </div>
          <div class="mt-5 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <div
              v-for="stat in streamStats"
              :key="stat.label"
              class="stat-tile motion-card"
              :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''"
            >
              <div class="text-[11px] uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                {{ stat.label }}
              </div>
              <div class="stat-value">{{ stat.value }}</div>
              <div
                class="text-xs"
                :class="[
                  isNight ? 'text-meow-night-soft' : 'text-meow-soft',
                  stat.tone === 'warn' ? 'text-[#e06b8b]' : ''
                ]"
              >
                {{ stat.note }}
              </div>
            </div>
          </div>
        </section>

        <section id="ingest" class="mt-14 grid gap-6 md:grid-cols-[1.1fr_0.9fr]">
          <div
            class="meow-window meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line text-meow-night-soft' : 'text-meow-soft'"
          >
            <div class="meow-window-bar">
              <span class="meow-window-dots"></span>
              <span class="meow-window-title">推流配置</span>
              <div class="flex items-center gap-2">
                <button
                  class="meow-pill motion-press px-2 py-0.5 text-[11px]"
                  type="button"
                  :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''"
                  @click="refreshIngestToken"
                >
                  刷新推流 Key
                </button>
                <button
                  class="meow-pill motion-press px-2 py-0.5 text-[11px]"
                  type="button"
                  :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''"
                  @click="loadAll"
                >
                  刷新地址
                </button>
              </div>
            </div>
            <div class="meow-window-body">
              <div class="mt-3 space-y-3">
                <div v-for="item in ingestDisplay" :key="item.url || item.value" class="meow-window-item">
                  <div class="meow-window-time">{{ item.protocol || item.label }}</div>
                  <div>
                    <div class="meow-window-titleline">
                      <span>{{ getDisplayValue(item) }}</span>
                      <span class="meow-pill">{{ item.tag }}</span>
                    </div>
                    <div class="meow-window-meta">{{ item.note }}</div>
                    <div class="mt-2 flex flex-wrap items-center gap-2 text-[11px]">
                      <button
                        class="copy-btn"
                        type="button"
                        @click="copyText(item.url || item.value)"
                      >
                        复制
                      </button>
                      <button
                        v-if="isKeyItem(item)"
                        class="copy-btn"
                        type="button"
                        @click="showKey = !showKey"
                      >
                        {{ showKey ? "隐藏 Key" : "显示 Key" }}
                      </button>
                      <span v-if="copyNotice" class="text-[#e06b8b]">{{ copyNotice }}</span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <div
            class="meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <h3 class="font-display text-xl">推流健康度</h3>
            <p class="mt-2 text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
              实时检测链路瓶颈，建议切换节点或调整码率。
            </p>
            <div class="mt-4 space-y-4">
              <div v-for="check in healthChecks" :key="check.label" class="space-y-2">
                <div class="flex items-center justify-between text-sm">
                  <span>{{ check.label }}</span>
                  <span class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">{{ check.note }}</span>
                </div>
                <div class="health-bar" :class="isNight ? 'bg-meow-night-line/50' : ''">
                  <div class="health-bar-fill" :style="{ width: check.value + '%' }"></div>
                </div>
              </div>
            </div>
            <div class="stream-divider"></div>
            <div class="grid gap-3 sm:grid-cols-2">
              <div class="stat-tile" :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''">
                <div class="text-[11px] uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">备用推流</div>
                <div class="text-base font-600">待启用</div>
                <div class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">建议开启双路推流</div>
              </div>
              <div class="stat-tile" :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''">
                <div class="text-[11px] uppercase tracking-widest" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">线路自检</div>
                <div class="text-base font-600">已完成</div>
                <div class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">下次检测 20:30</div>
              </div>
            </div>
          </div>
        </section>

        <section id="smtp" class="mt-14">
          <div
            class="meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex items-center justify-between gap-4">
              <div>
                <h3 class="font-display text-xl">SMTP 设置</h3>
                <p class="mt-2 text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  用于观众邮箱验证码与通知发送。
                </p>
              </div>
              <button class="meow-btn-ghost motion-press" type="button" @click="loadSmtp">
                刷新
              </button>
            </div>
            <div class="mt-5 grid gap-4 md:grid-cols-2">
              <div class="field-group">
                <label class="field-label">SMTP Host</label>
                <input v-model="smtpConfig.host" class="field-input" placeholder="smtp.example.com" />
              </div>
              <div class="field-group">
                <label class="field-label">SMTP Port</label>
                <input v-model="smtpConfig.port" class="field-input" type="number" placeholder="587" />
              </div>
              <div class="field-group">
                <label class="field-label">SMTP 用户名</label>
                <input v-model="smtpConfig.username" class="field-input" placeholder="user@example.com" />
              </div>
              <div class="field-group">
                <label class="field-label">SMTP 密码</label>
                <input v-model="smtpConfig.password" class="field-input" type="password" :placeholder="smtpConfig.passwordSet ? '已设置，留空保持不变' : '输入密码'" />
              </div>
              <div class="field-group">
                <label class="field-label">发件人地址</label>
                <input v-model="smtpConfig.from" class="field-input" placeholder="no-reply@example.com" />
              </div>
              <div class="field-group">
                <label class="field-label">Reply-To（可选）</label>
                <input v-model="smtpConfig.replyTo" class="field-input" placeholder="support@example.com" />
              </div>
            </div>
            <div class="mt-4 flex flex-wrap items-center gap-3">
              <label class="meow-pill motion-press">
                <input type="checkbox" class="mr-2" v-model="smtpConfig.starttls" />
                启用 STARTTLS
              </label>
              <button class="meow-btn-primary motion-press" type="button" :disabled="smtpSaving" @click="saveSmtp">
                保存 SMTP
              </button>
              <div class="flex flex-wrap items-center gap-2">
                <input v-model="smtpTestTo" class="field-input min-w-[200px]" placeholder="测试邮箱" />
                <button class="meow-btn-ghost motion-press" type="button" :disabled="smtpTestSending" @click="sendSmtpTest">
                  发送测试邮件
                </button>
              </div>
              <span v-if="smtpNotice" class="text-xs text-[#e06b8b]">{{ smtpNotice }}</span>
            </div>
          </div>
        </section>

        <section id="notify" class="mt-14">
          <div
            class="meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex items-center justify-between gap-4">
              <div>
                <h3 class="font-display text-xl">通知模板</h3>
                <p class="mt-2 text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  支持占位符与正则规则，自动拼接直播地址。
                </p>
              </div>
              <button class="meow-btn-ghost motion-press" type="button" @click="loadNotifyTemplates">
                刷新
              </button>
            </div>
            <div class="mt-5 grid gap-4 md:grid-cols-2">
              <div class="field-group">
                <label class="field-label">直播地址（liveUrl）</label>
                <input v-model="notifyTemplates.live_url" class="field-input" placeholder="https://live.example.com" />
              </div>
            </div>
            <div class="mt-5 grid gap-4 md:grid-cols-3">
              <div class="field-group">
                <label class="field-label">开播标题</label>
                <input v-model="notifyTemplates.live.title" class="field-input" placeholder="主播已开播" />
              </div>
              <div class="field-group md:col-span-2">
                <label class="field-label">开播内容</label>
                <textarea v-model="notifyTemplates.live.message" class="field-input min-h-[80px]" placeholder="直播间已开始：{title}"></textarea>
              </div>
              <div class="field-group">
                <label class="field-label">开播 URL（可选）</label>
                <input v-model="notifyTemplates.live.url" class="field-input" placeholder="{liveUrl}" />
              </div>
            </div>
            <div class="mt-4 grid gap-4 md:grid-cols-3">
              <div class="field-group">
                <label class="field-label">下播标题</label>
                <input v-model="notifyTemplates.offline.title" class="field-input" placeholder="主播已下播" />
              </div>
              <div class="field-group md:col-span-2">
                <label class="field-label">下播内容</label>
                <textarea v-model="notifyTemplates.offline.message" class="field-input min-h-[80px]" placeholder="直播已结束：{title}"></textarea>
              </div>
              <div class="field-group">
                <label class="field-label">下播 URL（可选）</label>
                <input v-model="notifyTemplates.offline.url" class="field-input" placeholder="" />
              </div>
            </div>
            <div class="mt-4 grid gap-4 md:grid-cols-3">
              <div class="field-group">
                <label class="field-label">排期标题</label>
                <input v-model="notifyTemplates.schedule.title" class="field-input" placeholder="排期已更新" />
              </div>
              <div class="field-group md:col-span-2">
                <label class="field-label">排期内容</label>
                <textarea v-model="notifyTemplates.schedule.message" class="field-input min-h-[80px]" placeholder="下一场：{scheduleTime} {scheduleTitle}"></textarea>
              </div>
              <div class="field-group">
                <label class="field-label">排期 URL（可选）</label>
                <input v-model="notifyTemplates.schedule.url" class="field-input" placeholder="" />
              </div>
            </div>
            <div class="mt-4 field-group">
              <label class="field-label">规则（JSON 数组）</label>
              <textarea v-model="notifyRulesRaw" class="field-input min-h-[140px]" placeholder='[{ "kind": "live", "field": "title", "pattern": "(.*)", "title": "开播：$1", "message": "标题：$1 {liveUrl}" }]'></textarea>
            </div>
            <div class="mt-4 flex flex-wrap items-center gap-3">
              <button class="meow-btn-primary motion-press" type="button" :disabled="notifySaving" @click="saveNotifyTemplates">
                保存模板
              </button>
              <span v-if="notifyNotice" class="text-xs text-[#e06b8b]">{{ notifyNotice }}</span>
            </div>
          </div>
        </section>

        <section id="telegram" class="mt-14">
          <div
            class="meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex items-center justify-between gap-4">
              <div>
                <h3 class="font-display text-xl">Telegram 通知</h3>
                <p class="mt-2 text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  配置频道后，直播与排期更新会推送到 Telegram。
                </p>
              </div>
              <button class="meow-btn-ghost motion-press" type="button" @click="loadTelegram">
                刷新
              </button>
            </div>
            <div class="mt-5 grid gap-4 md:grid-cols-2">
              <div class="field-group">
                <label class="field-label">频道</label>
                <input v-model="telegramConfig.channel" class="field-input" placeholder="@channel 或 https://t.me/xxx" />
              </div>
              <div class="field-group">
                <label class="field-label">Bot 状态</label>
                <div class="text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  {{ telegramConfig.tokenConfigured ? "TELEGRAM_BOT_TOKEN 已配置" : "请在后端配置 TELEGRAM_BOT_TOKEN" }}
                </div>
              </div>
            </div>
            <div class="mt-4 flex flex-wrap items-center gap-3">
              <button class="meow-btn-primary motion-press" type="button" :disabled="telegramSaving" @click="saveTelegram">
                保存 Telegram
              </button>
              <span v-if="telegramNotice" class="text-xs text-[#e06b8b]">{{ telegramNotice }}</span>
            </div>
          </div>
        </section>

        <section id="anti-abuse" class="mt-14">
          <div
            class="meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex items-center justify-between gap-4">
              <div>
                <h3 class="font-display text-xl">防盗刷设置</h3>
                <p class="mt-2 text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  观众邮箱验证码的频率限制与邮箱规则控制。
                </p>
              </div>
              <button class="meow-btn-ghost motion-press" type="button" @click="loadViewerAntiAbuse">
                刷新
              </button>
            </div>
            <div class="mt-5 grid gap-4 md:grid-cols-2">
              <div class="field-group">
                <label class="field-label">限流窗口（秒）</label>
                <input v-model="antiAbuseConfig.verifyEmailRateLimitWindowSecs" class="field-input" type="number" placeholder="1800" />
              </div>
              <div class="field-group">
                <label class="field-label">窗口内 IP 最大发送次数</label>
                <input v-model="antiAbuseConfig.verifyEmailRateLimitMax" class="field-input" type="number" placeholder="3" />
              </div>
              <div class="field-group">
                <label class="field-label">窗口内单邮箱最大发送次数</label>
                <input v-model="antiAbuseConfig.verifyEmailRateLimitEmailMax" class="field-input" type="number" placeholder="2" />
              </div>
              <div class="field-group">
                <label class="field-label">冷却时间（秒）</label>
                <input v-model="antiAbuseConfig.verifyEmailCooldownSecs" class="field-input" type="number" placeholder="600" />
              </div>
              <div class="field-group">
                <label class="field-label">注册令牌有效期（秒）</label>
                <input v-model="antiAbuseConfig.registerTokenTtlSecs" class="field-input" type="number" placeholder="600" />
              </div>
              <div class="field-group">
                <label class="field-label">邮箱策略</label>
                <div class="flex flex-wrap items-center gap-3">
                  <label class="meow-pill motion-press">
                    <input type="checkbox" class="mr-2" v-model="antiAbuseConfig.blockDisposableEmail" />
                    拦截一次性邮箱
                  </label>
                  <label class="meow-pill motion-press">
                    <input type="checkbox" class="mr-2" v-model="antiAbuseConfig.blockEduGovEmail" />
                    拦截 .edu/.gov
                  </label>
                </div>
              </div>
            </div>
            <div class="mt-4 flex flex-wrap items-center gap-3">
              <button class="meow-btn-primary motion-press" type="button" :disabled="antiAbuseSaving" @click="saveViewerAntiAbuse">
                保存防盗刷配置
              </button>
              <span v-if="antiAbuseNotice" class="text-xs text-[#e06b8b]">{{ antiAbuseNotice }}</span>
            </div>
          </div>
        </section>

        <section id="schedule" class="mt-14">
          <div class="flex flex-wrap items-center justify-between gap-3">
            <h2 class="font-display text-2xl">今日直播排期</h2>
            <span class="meow-pill">管理端可编辑</span>
          </div>
          <div
            class="meow-window meow-card motion-card mt-4 p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line text-meow-night-soft' : 'text-meow-soft'"
          >
            <div class="meow-window-bar">
              <span class="meow-window-dots"></span>
              <span class="meow-window-title">排期编辑</span>
              <div class="flex items-center gap-2">
                <button class="meow-pill motion-press px-2 py-0.5 text-[11px]" type="button" @click="addScheduleItem">
                  新增条目
                </button>
                <button class="meow-pill motion-press px-2 py-0.5 text-[11px]" type="button" :disabled="!scheduleDirty" @click="resetScheduleDraft">
                  重置
                </button>
                <button class="meow-pill motion-press px-2 py-0.5 text-[11px]" type="button" :disabled="scheduleSaving" @click="saveSchedule">
                  保存排期
                </button>
              </div>
            </div>
            <div class="meow-window-body">
              <div class="mt-3 grid gap-3">
                <div v-for="(item, index) in scheduleDraft" :key="`${item.time}-${index}`" class="meow-window-item">
                  <div class="meow-window-time">
                    <input v-model="item.time" class="field-input !h-8 !text-xs" placeholder="20:00" @input="scheduleDirty = true" />
                  </div>
                  <div class="w-full">
                    <div class="grid gap-2 md:grid-cols-[1.4fr_0.6fr_0.8fr]">
                      <input v-model="item.title" class="field-input !h-8 !text-xs" placeholder="节目标题" @input="scheduleDirty = true" />
                      <input v-model="item.tag" class="field-input !h-8 !text-xs" placeholder="标签" @input="scheduleDirty = true" />
                      <input v-model="item.host" class="field-input !h-8 !text-xs" placeholder="主持人" @input="scheduleDirty = true" />
                    </div>
                    <div class="mt-2 flex items-center gap-2 text-[11px]">
                      <button class="copy-btn" type="button" @click="removeScheduleItem(index)">删除</button>
                    </div>
                  </div>
                </div>
              </div>
              <div class="mt-3 text-xs text-[#e06b8b]" v-if="scheduleNotice">{{ scheduleNotice }}</div>
            </div>
          </div>
        </section>

        <section id="channels" class="mt-14">
          <div class="flex flex-wrap items-center justify-between gap-3">
            <h2 class="font-display text-2xl">频道管理</h2>
            <span class="meow-pill">{{ channelCards.length }} 个频道在线</span>
          </div>
          <div class="mt-5 grid gap-4 md:grid-cols-3">
            <article
              v-for="channel in channelCards"
              :key="channel.name"
              class="meow-card motion-card p-5"
              :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
            >
              <div class="flex items-center justify-between">
                <h3 class="text-base font-600">{{ channel.name }}</h3>
                <span class="meow-pill">{{ channel.status }}</span>
              </div>
              <p class="mt-3 text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                {{ channel.category }}
              </p>
              <div class="mt-4 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                关注数 {{ channel.followers }}
              </div>
            </article>
          </div>
        </section>

        <section id="ops" class="mt-14 grid gap-6 md:grid-cols-2">
          <div
            class="meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex items-center justify-between">
              <h3 class="font-display text-xl">运营提醒</h3>
              <span class="meow-pill">实时</span>
            </div>
            <div class="mt-4 space-y-3">
              <div
                v-for="alert in opsAlerts"
                :key="alert.title"
                class="stat-tile"
                :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''"
              >
                <div class="text-sm font-600">{{ alert.title }}</div>
                <p class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  {{ alert.desc }}
                </p>
              </div>
            </div>
          </div>

          <div
            class="meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex items-center justify-between">
              <h3 class="font-display text-xl">推荐节点</h3>
              <span class="meow-pill">智能调度</span>
            </div>
            <p class="mt-2 text-sm" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
              系统根据地区与负载推荐最优接入点。
            </p>
            <div class="mt-4 space-y-3">
              <div
                v-for="node in ingestNodes"
                :key="node.city"
                class="stat-tile"
                :class="isNight ? 'border-meow-night-line bg-meow-night-bg text-meow-night-ink' : ''"
              >
                <div class="flex items-center justify-between text-sm">
                  <span>{{ node.city }}</span>
                  <span class="meow-pill">{{ node.load }}负载</span>
                </div>
                <div class="mt-1 text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">延迟 {{ node.latency }}</div>
              </div>
            </div>
          </div>
        </section>

        <section class="mt-14">
          <div
            class="meow-card motion-card p-5"
            :class="isNight ? 'bg-meow-night-card/80 border-meow-night-line' : ''"
          >
            <div class="flex items-center justify-between">
              <h3 class="font-display text-xl">事件流</h3>
              <button
                class="meow-pill motion-press text-[11px]"
                type="button"
                @click="clearEvents"
              >
                清空
              </button>
            </div>
            <div class="event-log mt-3">
              <div v-if="eventLog.length === 0" class="text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                暂无事件
              </div>
              <div v-for="item in eventLog" :key="item.id" class="event-item">
                <div class="event-title">{{ item.title }}</div>
                <div class="event-detail" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  {{ item.detail }}
                </div>
                <div class="event-time" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
                  {{ item.time }}
                </div>
              </div>
            </div>
          </div>
        </section>

        <footer class="mt-16 text-center text-xs" :class="isNight ? 'text-meow-night-soft' : 'text-meow-soft'">
          © 2026 Meowhuan Live Studio. 保持稳定，保持温柔。
        </footer>
      </div>
    </div>
  </div>
</template>
