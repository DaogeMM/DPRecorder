// player-api.js —— 播放页侧的接口薄壳（ESM，由 index.html / player.html 以 module 引入）。
//
// 三件事：
//   1. BGM 桥：wasm 里的音频后端是空实现（`sasa` 在浏览器里不干活），音乐由一个不在
//      版面上的 `<audio>` 播，每 120ms 用 `wm_time()`（引擎的谱面时间）把 `currentTime`
//      拉回对齐——和单页版 `tools/pack.mjs` 里那段桥是同一套做法：静音跟随 + 偏差超过
//      `RESYNC` 才硬跳，避免每个 tick 都掐断音频。
//   2. `window.DPRWPlayer`：同源嵌入时宿主用 `DPRW.attach(win)` 直接调这里的函数。
//   3. postMessage 通道：跨域 iframe 嵌入时宿主发 `{dprw:{cmd,value}}`，这里回
//      `{dprw:{event,value}}`。页面对来源做白名单：`?origin=a,b` 逗号分隔，缺省 `*`。
//
// 资源前缀：`?assets=` 覆盖页面侧音频（BGM、按键音效）的目录；wasm 引擎内部的资源
// （`font.ttf` / `SP.json` / `respack/` …）由 macroquad 按 `assets/` 固定拼，页面改不了。
import * as dps from "./wasm/dps_player.js";

const params = new URLSearchParams(location.search);

/// 页面侧音频目录，末尾保证有 `/`。
const assetRoot = (() => {
  const raw = params.get("assets");
  if (!raw) return "./assets/";
  return raw.endsWith("/") ? raw : raw + "/";
})();
// 主脚本拼按键音效路径时读它（pack-web 把那一行改写成读这个全局）。
window.DPRW_ASSET_ROOT = assetRoot;

/// 允许发命令的来源；`*` 表示不校验（README 里有说明）。
const ALLOW = String(params.get("origin") || "*")
  .split(",")
  .map((it) => it.trim())
  .filter(Boolean);

/// `postMessage` 的 targetOrigin：`file://` 页面的来源是字符串 `"null"`，不能当 targetOrigin 用。
const safeTarget = (origin) => (origin && origin !== "null" ? origin : "*");

const originAllowed = (origin) => ALLOW.includes("*") || ALLOW.includes(origin);

// ─── 事件总线 ─────────────────────────────────────────────────────────────────────

const listeners = new Map();

/// 订阅事件，返回取消订阅的函数。
function on(event, cb) {
  if (typeof cb !== "function") return () => {};
  let set = listeners.get(event);
  if (!set) listeners.set(event, (set = new Set()));
  set.add(cb);
  return () => set.delete(cb);
}

function emit(event, value) {
  const set = listeners.get(event);
  if (!set || !set.size) return;
  for (const cb of Array.from(set)) {
    try {
      cb(value);
    } catch (err) {
      console.error("[dprw] listener failed:", event, err);
    }
  }
}

// ─── BGM 桥 ──────────────────────────────────────────────────────────────────────

/// 引擎时间与音频位置的允许偏差；超过就硬跳。
const RESYNC = 0.03;

const bgm = {
  el: null,
  song: "music.ogg",
  /// 预览区间（谱面绝对时间，秒）；BGM 也跟着这个区间走。
  previewStart: 0,
  previewEnd: 0,
  rate: 1,
  hint: null,
};

/// 引擎的 `volume` 之类不在这里管，只认 `info.txt` 里这几行。
function parseInfo(text) {
  for (const line of text.split(/\r?\n/)) {
    const colon = line.indexOf(":");
    if (colon < 0) continue;
    const key = line.slice(0, colon).trim();
    const value = line.slice(colon + 1).trim();
    const num = Number(value);
    if ((key === "previewS" || key === "previewStart") && Number.isFinite(num) && num > 0) {
      bgm.previewStart = num;
    } else if ((key === "previewE" || key === "previewEnd") && Number.isFinite(num) && num > 0) {
      bgm.previewEnd = num;
    } else if (key === "Song" && value) {
      bgm.song = value;
    }
  }
}

/// 可播放区间的终点（谱面绝对时间）。`getTime()` / `seek()` 用的是同一套坐标，
/// 所以 `getDuration()` 返回的是「终点」而不是「长度」。
function computeDuration() {
  if (bgm.previewEnd > 0) return bgm.previewEnd;
  const el = bgm.el;
  const len = el && Number.isFinite(el.duration) ? el.duration : 0;
  if (len > 0) return bgm.previewStart + len;
  return 0;
}

/// 摘掉旧版挂上去的提示层（页面可能还留着上一份脚本建的节点）。
function hideHint() {
  const hint = document.querySelector("[data-dprw-hint]");
  if (hint) hint.remove();
  bgm.hint = null;
}

/// 引擎暂停 / 还没进游戏时不要出声。
function syncAudio() {
  const el = bgm.el;
  if (!el || !state.ready) return;
  // 引擎自己用 WebAudio 出声（音乐连同按键音都由引擎混），页面这条 BGM 桥就让位。
  if (engineHasAudio()) {
    if (!el.paused) el.pause();
    return;
  }
  let time = 0;
  let paused = true;
  try {
    time = dps.wm_time();
    paused = dps.wm_paused();
  } catch (err) {
    return;
  }
  if (paused) {
    if (!el.paused) el.pause();
    return;
  }
  // 谱面时间还是 0 说明引擎还在加载，别从曲首放起来。
  if (!(time > 0)) return;
  if (Math.abs(el.currentTime - time) > RESYNC) {
    try {
      el.currentTime = time;
    } catch (err) {
      // 元数据还没到时会抛，下一次 tick 再试。
      return;
    }
  }
  kickAudio();
}

/// 引擎是不是用浏览器的 WebAudio 直接出声。
///
/// `player.html` 里那段脚本把 `wm_audio_native()` 挂在全局上（`player-api.js` 比它先加载，
/// 所以只能等到调用时再取）。
function engineHasAudio() {
  const probe = globalThis.__wmAudioNative;
  if (typeof probe !== "function") return false;
  try {
    return probe() === true;
  } catch (err) {
    return false;
  }
}

/// 播一次；被自动播放策略拦下就记一条日志，不再挂提示层。
///
/// 提示层（「浏览器拦下了自动播放」）会盖住谱面画面，而引擎自己的音频走 WebAudio，
/// 本来就绕不过这条策略——挡画面换不来声音，所以只留日志。
function kickAudio() {
  const el = bgm.el;
  if (!el || !state.ready || el.paused === false) return;
  if (engineHasAudio()) return;
  const started = el.play();
  if (!started || typeof started.then !== "function") return;
  started.then(hideHint).catch((err) => {
    console.warn("[dprw] BGM 自动播放被拦:", err && err.name);
  });
}

async function initAudio() {
  try {
    const res = await fetch("./assets/info.txt");
    if (res.ok) parseInfo(await res.text());
  } catch (err) {
    console.warn("[dprw] 读不到 assets/info.txt，按默认曲目处理:", err);
  }
  const el = document.createElement("audio");
  el.src = assetRoot + bgm.song;
  el.preload = "auto";
  el.style.display = "none";
  // `?mute=1`：静音跟随（静音的音频不受浏览器自动播放拦截，也就不会挂出「点一下开始」的遮罩），
  // 编辑窗口的预览区用它——那一块是画面，不该被提示层盖住。
  if (params.get("mute")) el.muted = true;
  el.addEventListener("loadedmetadata", () => {
    if (state.ready && !state.duration) {
      state.duration = computeDuration();
      emitState();
    }
  });
  el.addEventListener("error", () => console.warn("[dprw] BGM 加载失败:", el.src));
  document.body.appendChild(el);
  bgm.el = el;
  setInterval(syncAudio, 120);
  // 任何一次点击 / 按键都再试一次播放，覆盖「提示层被别的元素挡住」的情况。
  document.addEventListener("pointerdown", kickAudio, true);
  document.addEventListener("keydown", kickAudio, true);
}

// ─── 引擎状态 ────────────────────────────────────────────────────────────────────

const state = { ready: false, duration: 0, error: "" };

/// 引擎还没 ready 时先把命令攒着，起来之后按顺序补做。
const pending = [];
function whenReady(fn) {
  if (state.ready) fn();
  else pending.push(fn);
}

/// 引擎「能收命令了」的判据是它的 `wm_status()` 文本（见 `bin/wasm_player.rs`）。
function pollReady() {
  if (state.ready) return;
  let status = "";
  try {
    status = dps.wm_status();
  } catch (err) {
    return; // 实例化还没结束，导出函数还调不到。
  }
  if (/^frame /.test(status) || status === "main ready" || status === "font ready") {
    markReady();
  } else if (status.startsWith("ERROR ")) {
    state.error = status;
    emit("error", status);
    broadcast("error", status);
  }
}

function markReady() {
  state.ready = true;
  state.duration = computeDuration();
  window.__dprwReady = true;
  // 别写成 `const info = info()`：同名的局部变量会把函数名遮住，落到 TDZ 上直接抛 ReferenceError。
  const readyInfo = info();
  emit("ready", readyInfo);
  broadcast("ready", readyInfo);
  emitState();
  const queued = pending.splice(0);
  for (const fn of queued) fn();
}

function info() {
  return {
    duration: state.duration,
    startTime: bgm.previewStart,
    previewEnd: bgm.previewEnd,
    song: bgm.song,
    rate: bgm.rate,
  };
}

function snapshot() {
  let time = 0;
  let paused = false;
  if (state.ready) {
    try {
      time = dps.wm_time();
      paused = dps.wm_paused();
    } catch (err) {
      // 忽略：下一帧就会好。
    }
  }
  return { time, paused, rate: bgm.rate, duration: state.duration };
}

// ─── 命令 ────────────────────────────────────────────────────────────────────────

function setPaused(want) {
  if (!state.ready) return;
  let paused = false;
  try {
    paused = dps.wm_paused();
  } catch (err) {
    return;
  }
  if (paused !== want) dps.wm_toggle_pause();
  // 引擎下一帧才换档，音频这一侧立刻跟，免得最多 120ms 内 BGM 还在响 / 还没起。
  if (want) {
    if (bgm.el && !bgm.el.paused) bgm.el.pause();
  } else {
    kickAudio();
  }
  emitState();
}

function play() {
  whenReady(() => setPaused(false));
}

function pause() {
  whenReady(() => setPaused(true));
}

function toggle() {
  whenReady(() => {
    let paused = false;
    try {
      paused = dps.wm_paused();
    } catch (err) {
      return;
    }
    setPaused(!paused);
  });
}

function seek(seconds) {
  const at = Number(seconds);
  if (!Number.isFinite(at)) return;
  whenReady(() => {
    const target = Math.max(0, at);
    dps.wm_seek(target);
    if (bgm.el) {
      try {
        bgm.el.currentTime = target;
      } catch (err) {
        // 元数据还没到，交给下一 tick 的 syncAudio。
      }
    }
    emit("seek", target);
    broadcast("seek", target);
    emit("time", target);
    broadcast("time", target);
    emitState();
  });
}

/// 换一份谱面：把新文件灌进本页的 `__dprw_files`（引擎的内存文件系统读这张 Map），
/// 再让引擎原地重载（`wasm_player.rs` 的 `wm_reload`）。播放头、播放态、音量都不动 ——
/// 编辑器改一个参数就调一次，画面不能跟着跳。
function reload(files) {
  const map = window.__dprw_files;
  if (map && files && typeof files === "object") {
    map.clear();
    for (const name of Object.keys(files)) map.set(name, files[name]);
  }
  whenReady(() => {
    try {
      dps.wm_reload();
    } catch (err) {
      console.warn("[dprw] 重载谱面失败:", err);
    }
  });
}

function setRate(rate) {
  const value = Number(rate);
  if (!Number.isFinite(value)) return;
  const clamped = Math.min(5, Math.max(0.1, value));
  whenReady(() => {
    dps.wm_set_rate(clamped);
    bgm.rate = clamped;
    if (bgm.el) {
      try {
        bgm.el.playbackRate = clamped;
        bgm.el.defaultPlaybackRate = clamped;
      } catch (err) {
        // playbackRate 只可能因为没有媒体资源失败，忽略。
      }
    }
    emit("rate", clamped);
    broadcast("rate", clamped);
    emitState();
  });
}

// ─── 音量 ────────────────────────────────────────────────────────────────────────

/// 音乐 / 音效音量（0~1）。改完先同步页面这两条桥，再交给引擎：
/// 引擎能自己出声时音乐走混音器放大器、音效每次播放读 `config.volume_sfx`，
/// 出不了声时页面 `<audio>` 与按键音效桥才是真正的声源。
const volume = { music: 0.5, sfx: 0.4 };

function clamp01(value) {
  const num = Number(value);
  if (!Number.isFinite(num)) return 1;
  return Math.min(1, Math.max(0, num));
}

function syncBridgeVolume() {
  const el = bgm.el;
  if (el) {
    el.volume = volume.music;
    // `?mute=1`：这条 BGM 桥整场不出声（编辑窗口的预览只当画面用，声音由引擎自己出）。
    // 开关在 `initAudio` 里读过一次，之后宿主调音量也只改 `volume`，不能把静音解开。
    if (!engineHasAudio()) el.muted = params.get("mute") ? true : volume.music <= 0.001;
  }
  // 按键音效桥由 player.html 那段脚本建（`__dprwSetSfxVolume` 是它挂出来的主增益）。
  const setSfx = globalThis.__dprwSetSfxVolume;
  if (typeof setSfx === "function") {
    try {
      setSfx(volume.sfx);
    } catch (err) {
      console.warn("[dprw] 设置音效音量失败:", err);
    }
  }
}

/// 设置音量：`setVolume({ music: 0.3 })` / `setVolume({ sfx: 0 })`，只给一档也行。
function setVolume(next) {
  if (!next || typeof next !== "object") return;
  if (next.music != null) volume.music = clamp01(next.music);
  if (next.sfx != null) volume.sfx = clamp01(next.sfx);
  syncBridgeVolume();
  whenReady(() => {
    try {
      dps.wm_set_volume(volume.music, volume.sfx);
    } catch (err) {
      // 老引擎没有这个导出：页面这条桥已经改过了，声音照样跟得上。
      return;
    }
    const snap = { ...volume };
    emit("volume", snap);
    broadcast("volume", snap);
  });
}

function getVolume() {
  return { ...volume };
}

function getTime() {
  if (!state.ready) return 0;
  try {
    return dps.wm_time();
  } catch (err) {
    return 0;
  }
}

function getPaused() {
  if (!state.ready) return false;
  try {
    return dps.wm_paused();
  } catch (err) {
    return false;
  }
}

function getDuration() {
  return state.duration;
}

// ─── 周期广播：时间 / 播放态 ─────────────────────────────────────────────────────

let lastTime = -1e9;
let lastPaused = null;

function emitState() {
  const snap = snapshot();
  emit("state", snap);
  broadcast("state", snap);
}

function tick() {
  if (!state.ready) return;
  const snap = snapshot();
  if (snap.paused !== lastPaused) {
    lastPaused = snap.paused;
    const event = snap.paused ? "pause" : "play";
    emit(event);
    broadcast(event);
    emitState();
  }
  if (Math.abs(snap.time - lastTime) >= 0.02) {
    lastTime = snap.time;
    emit("time", snap.time);
    broadcast("time", snap.time);
  }
  if (!state.duration) {
    const duration = computeDuration();
    if (duration) {
      state.duration = duration;
      emitState();
    }
  }
}

// ─── postMessage 通道 ───────────────────────────────────────────────────────────

/// 给发过命令的窗口（`event.source`）记一份来源，回消息时用它算 targetOrigin。
const hosts = [];
function rememberHost(source, origin) {
  if (!source || source === window) return;
  if (hosts.some((it) => it.source === source)) return;
  hosts.push({ source, origin: safeTarget(origin) });
}

function post(target, origin, event, value) {
  if (!target) return;
  try {
    target.postMessage({ dprw: { event, value } }, origin);
  } catch (err) {
    console.warn("[dprw] postMessage 失败:", err);
  }
}

/// 父窗口即使从来没发过命令也要收（常规 iframe 嵌入就是这种）。
function parentTarget() {
  if (ALLOW.includes("*")) return "*";
  return safeTarget(ALLOW[0]);
}

function broadcast(event, value) {
  const parent = window.parent;
  if (parent && parent !== window) post(parent, parentTarget(), event, value);
  for (const host of hosts) {
    if (host.source === parent) continue;
    post(host.source, host.origin, event, value);
  }
}

function onMessage(event) {
  const data = event.data;
  if (!data || typeof data !== "object" || !data.dprw) return;
  if (!originAllowed(event.origin)) {
    console.warn("[dprw] 忽略了来自", event.origin, "的命令（白名单：", ALLOW.join(","), "）");
    return;
  }
  rememberHost(event.source, event.origin);
  const cmd = data.dprw.cmd;
  const value = data.dprw.value;
  switch (cmd) {
    case "play":
      play();
      break;
    case "pause":
      pause();
      break;
    case "toggle":
      toggle();
      break;
    case "seek":
      seek(value);
      break;
    case "setRate":
      setRate(value);
      break;
    case "reload":
      reload(value);
      break;
    case "volume":
      setVolume(value);
      break;
    case "state":
      emitState();
      break;
    case "info":
      post(event.source, safeTarget(event.origin), "ready", info());
      break;
    case "hello":
      post(event.source, safeTarget(event.origin), "state", snapshot());
      if (state.ready) post(event.source, safeTarget(event.origin), "ready", info());
      break;
    default:
      console.warn("[dprw] 未知命令:", cmd);
  }
}

window.addEventListener("message", onMessage);

// ─── 对外接口（同源嵌入直接用它） ───────────────────────────────────────────────

window.DPRWPlayer = {
  version: 1,
  play,
  pause,
  toggle,
  seek,
  setRate,
  reload,
  setVolume,
  getVolume,
  getTime,
  getPaused,
  getDuration,
  getRate: () => bgm.rate,
  getInfo: info,
  ready: () => state.ready,
  on,
  /// 同源嵌入时宿主 `destroy()` 会调它：停音频、撤监听，引擎本身继续跑。
  destroy() {
    hideHint();
    if (bgm.el) {
      try {
        bgm.el.pause();
      } catch (err) {
        // 忽略。
      }
    }
    listeners.clear();
  },
};

setInterval(pollReady, 100);
setInterval(tick, 250);
initAudio().catch((err) => console.warn("[dprw] BGM 桥没起来:", err));
