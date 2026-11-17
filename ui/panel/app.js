"use strict";

/* 机器名称默认值：纯文本，两行之间用 | 分隔（也接受真实换行） */
const DEFAULT_CPU_NAME = "CPU";
const DEFAULT_GPU_NAME = "GPU";

const CONFIG = {
  /* 设备电量接口（EasyBluetooth 统一标准数据 API），也是设备区唯一的数据源。
   * battery 是 null 语义、还带 isCharging / deviceType，比从字符串里正则拆要准得多。
   * 不通（没装 / 没在设置里开启 / 改了端口 / 开了 Token 鉴权）时设备区留空槽。 */
  cpuName: DEFAULT_CPU_NAME,
  gpuName: DEFAULT_GPU_NAME,
  fanMax: 1600,
  ringR: 90,
  arcR: 122,
  /* 6 个设备槽位：只决定行数，不再带默认图标 ——
     图标完全由 deviceType / 名称关键词决定，认不出来就是未识别图标 */
  devices: [{}, {}, {}, {}, {}, {}]
};

/* 演示数据：index.html?demo=1 时使用，方便调版式 */
const DEMO = {
  cpuLoad: 6, cpuTemp: 40, cpuClock: 5.23, cpuVolt: 1.072, cpuPower: 46,
  gpuLoad: 9, gpuTemp: 44, gpuClock: 2.84, gpuVolt: 0.935, gpuPower: 93,
  fps: 0, fps1l: 200, frame: 4.2, res: "3840 x 2160", hz: "240 Hz", net: 0,
  fans: [1122, 2647, 777, 769, 759],
  fanPct: [53, 78, 24, 32, 28],
  disks: [
    { name: "C:", pct: 49, size: "903 / 1861 GiB" },
    { name: "D:", pct: 72, size: "1373 / 1907 GiB" },
    { name: "E:", pct: 21, size: "389 / 1863 GiB" }
  ],
  pkg: 50, hot: 44, mem: 54,
  devices: [
    { name: "ATK F1 PRO", pct: 70 },
    { name: "HyperX 飓风3S", pct: 0 },
    { name: "Flydigi 八爪鱼5", pct: 0 },
    { name: "QK80MK2", pct: 33 },
    { name: "iPhone 16 PM", pct: 0 },
    { name: "AirPods Pro", pct: 0 }
  ]
};

const DEMO_ON = new URLSearchParams(location.search).get("demo") === "1";

const $ = (id) => document.getElementById(id);
const stage = $("stage");

let orientation = 1;

const FAN_IDS    = [null, null, null, null, null];  // 每行当前对应的传感器 id
const FAN_LABELS = ["", "", "", "", ""];            // 每行传感器的原始标签

function renderFanNames() {
  for (let i = 0; i < 5; i++) {
    const name = FAN_LABELS[i] || "--";
    const el = $(`fan-name-${i + 1}`);
    if (el && el.textContent !== name) el.textContent = name;
  }
}

/* 温度圆环取哪一路传感器：
 * 3 = 封装温度 TCPUPKG（默认） / 1 = 中央处理器(CPU) TCPU / 2 = CPU 二极管 TCPUDIO */
let TEMPRING = 3;
const RING_CAP = { 1: "CPU Temp", 2: "CPU Diode", 3: "CPU Package" };

function renderRingCap() {
  const el = $("pkg-cap");
  if (el) el.textContent = RING_CAP[TEMPRING] || RING_CAP[1];
}

/* 电量组件样式：1 = 列表（默认） / 2 = 圆环 */
let DEVSTYLE = 1;

/* Hardware display names */

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;"
  }[c]));
}

/* 名称区可用宽度（与 .head-txt p 的 max-width 保持一致） */
const NAME_FONT = '400 16.5px "Segoe UI", "Microsoft YaHei UI", sans-serif';

let _measureCtx = null;
function textWidth(s) {
  if (!_measureCtx) _measureCtx = document.createElement("canvas").getContext("2d");
  _measureCtx.font = NAME_FONT;
  return _measureCtx.measureText(s).width;
}

/* 单行超宽 => 二分查找最长可容纳前缀 + 省略号，比 CSS 硬切好看 */
function fitLine(text, width) {
  if (!text || textWidth(text) <= width) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (textWidth(text.slice(0, mid) + "…") <= width) lo = mid;
    else hi = mid - 1;
  }
  return text.slice(0, lo).replace(/[\s|]+$/, "") + "…";
}

/* "AMD Ryzen7|9800X3D" -> "AMD Ryzen7<br>9800X3D"，最多两行 */
function nameToHtml(raw, width) {
  const text = String(raw == null ? "" : raw).replace(/\r\n?/g, "\n").trim();
  if (!text) return "";
  const lines = text
    .split(/\s*\|\s*|\n+/)
    .map((t) => t.trim())
    .filter(Boolean)
    .slice(0, 2);
  if (lines.length === 1 && textWidth(lines[0]) > width) {
    const parts = lines[0].split(/\s+/);
    let split = 1;
    while (split + 1 < parts.length && textWidth(parts.slice(0, split + 1).join(" ")) <= width) split++;
    if (parts.length > 1) lines = [parts.slice(0, split).join(" "), parts.slice(split).join(" ")];
  }
  return lines.map(line => fitLine(line, width)).map(escapeHtml).join("<br>");
}

function renderNames() {
  const cpu = $("cpu-name");
  const gpu = $("gpu-name");
  if (cpu) cpu.innerHTML = nameToHtml(CONFIG.cpuName || DEFAULT_CPU_NAME, parseFloat(getComputedStyle(cpu).width));
  if (gpu) {
    const name = (CONFIG.gpuName || DEFAULT_GPU_NAME)
      .replace(/^(AMD Radeon|NVIDIA GeForce|Intel Arc)\s+(?=(?:RX|RTX|GTX)\s+\d|[AB]\d)/i, "$1|");
    gpu.innerHTML = nameToHtml(name, parseFloat(getComputedStyle(gpu).width));
  }
  if (cpu) cpu.title = CONFIG.cpuName;
  if (gpu) gpu.title = CONFIG.gpuName;
}

/* 用户属性里留空 => 保持默认，避免把名字清成空白 */
function setUserName(kind, value) {
  const text = value == null ? "" : String(value).trim();
  if (!text) return;
  if (kind === "cpu") CONFIG.cpuName = text;
  else if (kind === "gpu") CONFIG.gpuName = text;
  else return;
  renderNames();
}

function fitStage() {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  orientation = vw > vh ? 2 : 1;
  document.body.classList.toggle("landscape", orientation === 2);
  const w = orientation === 2 ? 2560 : 1440;
  const h = orientation === 2 ? 1440 : 2560;
  const scale = Math.min(vw / w, vh / h);
  stage.style.transform = `translate(${(vw - w * scale) / 2}px, ${(vh - h * scale) / 2}px) scale(${scale})`;
}

function clamp(n, min, max) {
  return Math.max(min, Math.min(max, n));
}

function setText(id, value, digits = 0) {
  const el = $(id);
  if (!el) return;
  el.textContent = value == null ? "--" : Number(value).toFixed(digits);
}

let storageSignature;
function renderStorage(volumes) {
  const list = $("store-list");
  const template = $("store-row-template");
  if (!list || !template) return;
  const signature = JSON.stringify(volumes);
  if (signature === storageSignature) return;
  storageSignature = signature;
  list.classList.toggle("compact", volumes.length > 3);
  while (list.childElementCount > volumes.length) list.lastElementChild.remove();
  while (list.childElementCount < volumes.length) list.append(template.content.cloneNode(true));
  for (const [index, volume] of volumes.entries()) {
    const row = list.children[index];
    row.querySelector(".store-line span").textContent = volume.name;
    row.querySelector(".store-line b").textContent = `${Math.round(volume.pct)}%`;
    row.querySelector(".store-size").textContent = volume.size;
    row.querySelector(".bar i").style.width = `${clamp(volume.pct, 0, 100)}%`;
  }
}

function setRaw(id, value) {
  const el = $(id);
  if (el) el.textContent = value == null || value === "" ? "--" : String(value);
}

/* 环形：0-100% */
function setRing(el, pct) {
  if (!el) return;
  const c = 2 * Math.PI * CONFIG.ringR;
  const p = clamp(pct == null ? 0 : pct, 0, 100);
  el.style.strokeDasharray = `${((c * p) / 100).toFixed(2)}px ${c.toFixed(2)}px`;
}

/* 温度弧：30-90℃ 映射到 270° */
function setArc(el, temp) {
  if (!el) return;
  const c = 2 * Math.PI * CONFIG.arcR;
  if (temp == null) {
    el.style.strokeDasharray = `0px ${c.toFixed(2)}px`;
    return;
  }
  const p = clamp((temp - 30) / 60, 0, 1);
  el.style.strokeDasharray = `${(c * 0.75 * p).toFixed(2)}px ${c.toFixed(2)}px`;
}

function setBar(id, pct) {
  const el = $(id);
  if (!el) return;
  el.style.width = `${clamp(pct == null ? 0 : pct, 0, 100)}%`;
}

function numeric(sensors, id) {
  const v = sensors && sensors[id] ? sensors[id].value : null;
  if (v == null || v === "") return null;
  const n = parseFloat(String(v).replace(/[^0-9.+-]/g, ""));
  return Number.isFinite(n) ? n : null;
}

function pick(sensors, ...ids) {
  for (const id of ids) {
    const n = numeric(sensors, id);
    if (n != null) return n;
  }
  return null;
}

function raw(sensors, id) {
  const v = sensors && sensors[id] ? sensors[id].value : null;
  return v == null || v === "" ? null : String(v);
}

/* 取整；取不到就返回 null（渲染成 "--"）。HWiNFO 的帧率带小数，AIDA64 是整数。 */
function round0(v) {
  return v == null ? null : Math.round(v);
}

function cpuLoadOf(sensors) {
  const total = numeric(sensors, "SCPUUTI");
  if (total != null) return total;
  const cores = [];
  for (let i = 1; i <= 32; i++) {
    const n = numeric(sensors, `SCPU${i}UTI`);
    if (n != null) cores.push(n);
  }
  if (!cores.length) return null;
  return cores.reduce((a, b) => a + b, 0) / cores.length;
}

/* 按名称关键词猜设备类型；返回 null 表示没认出来（交给槽位默认图标兜底）。
   顺序有讲究：耳机/耳塞要先于鼠标，"g pro x" 是耳机而 "g pro" 是鼠标。 */
function iconFor(name) {
  const s = String(name || "").toLowerCase();
  if (!s || s === "--") return null;

  if (/airpods|buds|earbud|freebuds|wf-1000|enco|tws/.test(s)) return "buds";
  if (/hyperx|headset|飓风|arctis|arctic|wh-1000|quietcomfort|qc\d|g pro x|耳机|耳麦/.test(s)) return "headset";
  if (/flydigi|八爪鱼|gamepad|controller|xbox|dualsense|dualshock|手柄|飞智|北通/.test(s)) return "gamepad";
  if (/keyboard|键盘|keychron|wooting|qk\d|akko|hhkb|filco|nj\d|rk\d|gmk|铝坨坨/.test(s)) return "keyboard";
  if (/iphone|phone|手机|galaxy|pixel|redmi|xiaomi|huawei|mate|oneplus|一加|小米|华为/.test(s)) return "phone";
  if (/mouse|鼠标|viper|deathadder|basilisk|g502|g304|g pro|gpw|model o|zowie|fk\d|ec\d|atk|罗技|雷蛇|logitech|razer/.test(s)) return "mouse";

  return null;
}

/* Windows exposes a battery percentage on supported present devices. */
const DEVICE_API = { list: [], reachable: false };
/* deviceType → 本地图标。general 表示接口没认出类别，这时还得靠设备名去猜，
 * 否则所有外设都会变成问号图标 */
const TYPE_ICON = {
  headphones: "headset",
  mouse: "mouse",
  keyboard: "keyboard",
  gamepad: "gamepad",
  speaker: "device"
};

/* 图标 id → 中文类别名。圆环样式上方那行大字用它。
 * buds 也归到「耳机」，和 headset 一致（用户只列了耳机这一档）。 */
const TYPE_LABEL = {
  headset: "耳机",
  buds: "耳机",
  mouse: "鼠标",
  keyboard: "键盘",
  gamepad: "手柄",
  phone: "手机",
  device: "通用设备"
};

/* headphones 是个大类：AirPods / TWS 这类入耳式也会被接口报成它，
 * 所以光看 deviceType 会全部画成头戴式耳机。名称能明确认出是耳塞
 * （airpods / buds / earbud / freebuds / wf-1000 / enco / tws）才换成 buds 图标，
 * 认不出来仍用头戴式。TYPE_LABEL 里 buds 也映射到「耳机」，圆环那行大字不变。 */
function isEarbud(item) {
  return !!item && iconFor(item.name) === "buds";
}

/* 图标只由「接口给的 deviceType」和「名称关键词」决定，
 * 都认不出来就落到未识别图标（#i-device：圆角方块 + 表情）。
 * 原来还有一层「槽位默认图标」兜底，会让乱名设备显示成鼠标/键盘，容易误导，已删。 */
function pickIcon(item) {
  const t = String(item.type || "").toLowerCase();
  const mapped = TYPE_ICON[t];
  if (mapped) return mapped === "headset" && isEarbud(item) ? "buds" : mapped;
  return (item.name && iconFor(item.name)) || "device";
}

/* 两条数据源统一成 { name, pct, charging, type }，下游渲染不用区分来源 */
const deviceConnections = new Map([
  ["usb", ["i-usb-interface", "USB 设备在线"]],
  ["usb_interface", ["i-usb-interface", "USB 接口在线。通过接收器连接时，手柄本体的无线状态未知。"]],
  ["usb_receiver", ["i-usb-receiver", "USB 接收器在线，外设本体连接状态未知。"]],
  ["bluetooth", ["i-bluetooth", "蓝牙已连接"]],
]);

function buildDevices() {
  return DEVICE_API.list.filter(item => item.name?.trim() && (Number.isFinite(item.pct) || deviceConnections.has(item.connection)))
    .sort((a, b) => Number(Number.isFinite(b.pct)) - Number(Number.isFinite(a.pct)));
}
function syncDeviceScrollControls(list) {
  const controls = $("device-scroll-controls");
  const max = list.scrollHeight - list.clientHeight;
  controls.hidden = max <= 1;
  list.classList.toggle("can-scroll", !controls.hidden);
  controls.children[0].disabled = list.scrollTop <= 1;
  controls.children[1].disabled = list.scrollTop >= max - 1;
}

function bindDeviceScroll(list) {
  const [up, down] = $("device-scroll-controls").children;
  for (const [button, direction] of [[up, -1], [down, 1]]) {
    button.addEventListener("click", () => {
      const step = list.clientHeight - (list.firstElementChild?.offsetHeight ?? 0);
      list.scrollBy({ top: step * direction, behavior: "smooth" });
    });
  }
  list.addEventListener("scroll", () => syncDeviceScrollControls(list));

  let drag;
  list.addEventListener("mousedown", event => {
    if (event.button !== 0 || list.scrollHeight <= list.clientHeight) return;
    const scale = list.getBoundingClientRect().height / list.offsetHeight;
    if (!scale) return;
    drag = { y: event.clientY, top: list.scrollTop, scale };
    list.classList.add("is-dragging");
  });
  document.addEventListener("mousemove", event => {
    if (drag) list.scrollTop = drag.top + (drag.y - event.clientY) / drag.scale;
  });
  const endDrag = () => {
    drag = undefined;
    list.classList.remove("is-dragging");
  };
  document.addEventListener("mouseup", endDrag);
  window.addEventListener("blur", endDrag);
}

function resizeDeviceRows(list, count) {
  while (list.childElementCount > count) list.lastElementChild.remove();
  while (list.childElementCount < count) {
    const row = document.createElement("div");
    row.className = "dev-row";

    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("class", "ico");
    svg.setAttribute("viewBox", "0 0 24 24");
    const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
    use.setAttribute("href", "#i-device");
    svg.appendChild(use);

    const name = document.createElement("span");
    name.className = "dev-name";

    const pct = document.createElement("span");
    pct.className = "dev-pct";

    const battery = document.createElement("span");
    battery.className = "battery";
    const fill = document.createElement("i");
    fill.style.width = "0%";
    battery.appendChild(fill);

    // 充电时叠在填充条上的闪电，用 iOS 原生的 bolt.fill 图（纯白、已裁掉留白）
    const bolt = document.createElement("img");
    bolt.className = "bolt";
    bolt.src = "icons/bolt.svg";
    bolt.alt = "";
    battery.appendChild(bolt);

    const connection = document.createElement("span");
    connection.className = "dev-connection";
    connection.setAttribute("role", "img");
    connection.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><use/></svg>';
    connection.hidden = true;

    row.append(svg, name, pct, battery, connection);
    list.appendChild(row);
  }
}

function updateDevices(list) {
  const container = $("device-list");
  if (!container) return;
  resizeDeviceRows(container, list.length);
  syncDeviceScrollControls(container);
  list.forEach((item, index) => {
    const row = container.children[index];
    const name = item.name.trim();
    const pct = item.pct;

    const hasCharge = Number.isFinite(pct);
    const pctText = hasCharge ? `${Math.round(pct)}%` : "";
    const connection = row.querySelector(".dev-connection");
    const connectionStyle = deviceConnections.get(item.connection);
    connection.hidden = hasCharge || !connectionStyle;
    if (!connection.hidden && connection.title !== connectionStyle[1]) {
      const [symbol, description] = connectionStyle;
      connection.title = description;
      connection.setAttribute("aria-label", description);
      connection.querySelector("use").setAttribute("href", "#" + symbol);
    }
    row.querySelector(".battery").hidden = !hasCharge;
    // 有 deviceType 就用它，general 或没给才回落到按名称猜，都认不出就是未识别图标
    const icon = pickIcon(item);
    const width = hasCharge ? `${Math.max(0, Math.min(100, pct))}%` : "0%";
    const isZero = pct === 0;
    const isLow = pct > 0 && pct <= 20;   // 0% 不算低电量，保持空框
    const isCharging = !!item.charging;
    // 充电且已经充满：电池图标整块换成薄荷青绿，和「在充电」的绿色区分开。
    // 用 round 后是否为 100 判断，跟上面显示的百分比文字保持一致
    // （接口报 99.6 时文字也是 100%）。
    const isFull = isCharging && Math.round(pct) >= 100;

    // 只在值真的变化时才碰 DOM。否则每秒重设 textContent / href / className
    // 会让浏览器反复重排重绘，传感器多返回一路时整块会闪一下
    const nameEl = row.querySelector(".dev-name");
    if (nameEl.textContent !== name) nameEl.textContent = name;

    const pctEl = row.querySelector(".dev-pct");
    if (pctEl.textContent !== pctText) pctEl.textContent = pctText;

    if (row.classList.contains("zero") !== isZero) row.classList.toggle("zero", isZero);
    if (row.classList.contains("low") !== isLow) row.classList.toggle("low", isLow);
    if (row.classList.contains("charging") !== isCharging) row.classList.toggle("charging", isCharging);
    if (row.classList.contains("full") !== isFull) row.classList.toggle("full", isFull);

    const use = row.querySelector("use");
    if (use) {
      const href = `#i-${icon}`;
      if (use.getAttribute("href") !== href) use.setAttribute("href", href);
    }

    const fill = row.querySelector(".battery i");
    if (fill && fill.style.width !== width) fill.style.width = width;
  });

  updateDevSource(list);
  renderRing(list);
}

/* ---------------- 圆环样式：只显示第一台设备 ---------------- */

/* 取设备的「图标 id + 中文类别」。接口的 deviceType 优先，
 * 没给（general）再按名称猜；都认不出来就落到通用设备 + i-device。 */
function ringKind(item) {
  const t = String(item.type || "").toLowerCase();
  const byType = TYPE_ICON[t];
  if (byType) {
    // 耳机里再分一层：AirPods 这类换耳塞图标，但 label 仍是「耳机」
    const icon = byType === "headset" && isEarbud(item) ? "buds" : byType;
    return { icon, label: TYPE_LABEL[icon] || "通用设备" };
  }

  const byName = iconFor(item.name);
  if (byName) return { icon: byName, label: TYPE_LABEL[byName] || "通用设备" };

  return { icon: "device", label: "通用设备" };
}

/* batteryLastUpdatedUtc 是 UTC（带 +00:00 偏移），转成本地 HH:MM:SS。
 * null / 解析失败都显示「无」。 */
function fmtUpdated(iso) {
  if (!iso) return "无";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return "无";
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

function renderRing(list) {
  const box = $("dev-ring");
  if (!box) return;

  // 第一台「有名字」的设备；一台都没有就整块留空
  const item = (list || []).find((d) => String(d.name || "").trim()) || null;
  const has = !!item;

  const name = has ? String(item.name).trim() : "--";
  const pct =
    has && item.pct != null && Number.isFinite(Number(item.pct))
      ? Number(item.pct)
      : null;
  const pctText = pct == null ? "--" : `${Math.round(pct)}%`;

  const kind = has ? ringKind(item) : { icon: "device", label: "通用设备" };

  // 状态：充电中 > 已休眠（有设备但读不到电量）> 在线；一台设备都没有就留 --
  let status = "--", charging = false;
  if (has) {
    if (item.charging) { status = "充电中"; charging = true; }
    else if (pct == null) { status = "已休眠"; }
    else { status = "在线"; }
  }

  // 局部写文本：和全局 setText（走 applyData 的那套）重名，所以单独起名
  const put = (id, text) => {
    const el = $(id);
    if (el && el.textContent !== text) el.textContent = text;
  };

  put("dr-type", kind.label);
  put("dr-name", name);
  put("dr-pct", pctText);
  put("dr-status-text", status);
  put("dr-time", `电量更新时间：${has ? fmtUpdated(item.updated) : "无"}`);

  const statusEl = $("dr-status");
  if (statusEl && statusEl.classList.contains("charging") !== charging) {
    statusEl.classList.toggle("charging", charging);
  }

  const use = $("dr-ico-use");
  if (use) {
    const href = `#i-${kind.icon}`;
    if (use.getAttribute("href") !== href) use.setAttribute("href", href);
  }

  // 进度弧：圆周长按实际半径算（r = (227 - 17.6) / 2 = 104.7）。
  // stroke-linecap: round 会让两端各多出半个环宽（W/2），所以：
  //   实线长度 = C × p - W，再用 dashoffset 把起点往前推 W/2，
  //   这样圆头的**外沿**刚好落在 12 点方向和进度角上，弧长不会被撑长。
  const bar = $("dr-ring-bar");
  if (bar) {
    const R = 104.7, W = 17.6;
    const C = 2 * Math.PI * R;
    const p = pct == null ? 0 : Math.max(0, Math.min(100, pct));
    // 满格单独处理：圆头会在 12 点叠出凸起，dasharray 的接缝也会露一条细线，
    // 所以干脆去掉 dasharray 直接画一个完整圆
    const full = p >= 99.5;
    if (full) {
      if (bar.getAttribute("stroke-dasharray") !== "none") {
        bar.setAttribute("stroke-dasharray", "none");
        bar.removeAttribute("stroke-dashoffset");
      }
    } else {
      const dash = p <= 0 ? 0 : Math.max(0.01, (C * p) / 100 - W);
      const arr = `${dash} ${C - dash}`;
      if (bar.getAttribute("stroke-dasharray") !== arr) {
        bar.setAttribute("stroke-dasharray", arr);
        bar.setAttribute("stroke-dashoffset", String(-W / 2));
      }
    }
    bar.style.opacity = p <= 0 ? "0" : "1";
    if (bar.classList.contains("full") !== full) bar.classList.toggle("full", full);
  }
}

/* 设备区自己的数据源提示，和右下角那条全局提示完全独立：
 *   接口可达                 → 不显示（正常状态）
 *   接口断了，但 30 秒保鲜期内还有旧数据 → 不显示（照常显示上次的好数据）
 *   接口断了，也没有可显示的数据       → 电量服务未连接
 * demo 模式没有真实接口，直接跳过。 */
function updateDevSource(list) {
  const el = $("dev-src");
  if (!el || DEMO_ON) return;

  const hasData = !!(list && list.some((d) => String(d.name || "").trim()));
  const text = hasData ? "" : DEVICE_API.reachable ? "暂无外设电量读数" : "电量采集暂无数据";

  if (el.textContent !== text) el.textContent = text;
  el.classList.toggle("on", !!text);
}

/* ---------------- chart ---------------- */

const MAX_POINTS = 300;
const series = { fps: [], cpu: [], gpu: [], mem: [], fps1l: [] };
let historyReady = false;

const canvas = $("chart");
const ctx = canvas ? canvas.getContext("2d") : null;

/* t 要留够 22px 标签的半高（11px）+ 余量，否则顶部的 360 会被画布裁掉；
   t 变大时 ch 变小，底部的 0 位置不变，等于只把顶部标签整体下移。
   b 要留够 X 轴时间标签的空间（17px 字 + 与 0 标签的间隙） */
const PAD = { l: 56, r: 0, t: 17, b: 34 };
const Y_MAX = 360;

function pushHistory(point) {
  if (!historyReady) {
    for (const key of Object.keys(series)) {
      series[key] = new Array(MAX_POINTS).fill(null);
      series[key][MAX_POINTS - 1] = point[key] ?? null;
    }
    historyReady = true;
    return;
  }
  for (const key of Object.keys(series)) {
    const arr = series[key];
    const prev = arr.length ? arr[arr.length - 1] : 0;
    arr.push(point[key] == null ? prev : point[key]);
    if (arr.length > MAX_POINTS) arr.shift();
  }
}

function resizeCanvas() {
  if (!canvas || !ctx) return;
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  if (!w || !h) return;
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = Math.max(1, Math.round(w * dpr));
  canvas.height = Math.max(1, Math.round(h * dpr));
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  drawChart();
}

function drawChart() {
  if (!canvas || !ctx) return;
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  if (!w || !h) return;

  const cw = w - PAD.l - PAD.r;
  const ch = h - PAD.t - PAD.b;

  ctx.clearRect(0, 0, w, h);
  ctx.font = '21px "Segoe UI", "Microsoft YaHei UI", sans-serif';
  ctx.textBaseline = "middle";

  for (const val of [0, 120, 240, 360]) {
    const y = PAD.t + ch - (val / Y_MAX) * ch;
    ctx.strokeStyle = "rgba(255, 255, 255, .30)";
    ctx.lineWidth = 1;
    ctx.setLineDash([]);
    ctx.beginPath();
    ctx.moveTo(PAD.l, y + .5);
    ctx.lineTo(PAD.l + cw, y + .5);
    ctx.stroke();

    ctx.fillStyle = "#F5F5F5";
    ctx.textAlign = "right";
    ctx.fillText(String(val), PAD.l - 8, y);
  }

  // 参考图只有两条竖向虚线，把绘图区等分成三段
  for (let i = 1; i <= 2; i++) {
    const x = PAD.l + (cw * i) / 3;
    // 竖向网格是虚线，比横向实线更亮，和参考图一致
    ctx.strokeStyle = "rgba(255, 255, 255, .50)";
    ctx.setLineDash([3.5, 4.5]);
    ctx.beginPath();
    ctx.moveTo(x + .5, PAD.t);
    ctx.lineTo(x + .5, PAD.t + ch);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  // X 轴时间刻度：每秒一个采样点，刻度落在整分钟上，随时间滚动，格式 HH:MM
  const spanSec = MAX_POINTS - 1;
  const nowMs = Date.now();
  const startMs = nowMs - spanSec * 1000;
  const axisY = PAD.t + ch + 25;
  ctx.font = '17px "Segoe UI", "Microsoft YaHei UI", sans-serif';
  ctx.fillStyle = "rgba(226, 234, 244, .68)";
  ctx.textBaseline = "middle";
  const pad2 = (n) => String(n).padStart(2, "0");
  let tickMs = Math.ceil(startMs / 60000) * 60000;
  while (tickMs <= nowMs) {
    const idx = (tickMs - startMs) / 1000;
    const x = PAD.l + (cw * idx) / spanSec;
    const d = new Date(tickMs);
    // 贴近两端时改用左/右对齐，避免标签被裁掉
    ctx.textAlign = x < PAD.l + 22 ? "left" : x > PAD.l + cw - 22 ? "right" : "center";

    ctx.strokeStyle = "rgba(226, 232, 240, .26)";
    ctx.beginPath();
    ctx.moveTo(x + .5, PAD.t + ch);
    ctx.lineTo(x + .5, PAD.t + ch + 5);
    ctx.stroke();

    ctx.fillText(`${pad2(d.getHours())}:${pad2(d.getMinutes())}`, x, axisY);
    tickMs += 60000;
  }

  if (!historyReady) return;

  const colors = { mem: "#FFFE91", gpu: "#9D7DFB", cpu: "#3dffa2", fps: "#488ff5", fps1l: "#72DAF0" };
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  // 绘制顺序即图层顺序：后画的压在上面。
  // FPS 垫底，往上依次 CPU、GPU，MEM 在最上层；1% L 只在 HWiNFO 数据源下追加，画在最上层
  const keys = ["fps", "cpu", "gpu", "mem"];
  keys.push("fps1l");
  for (const key of keys) {
    const arr = series[key];
    if (!arr || !arr.length) continue;
    ctx.beginPath();
    let continuous = false;
    for (let i = 0; i < arr.length; i++) {
      if (!Number.isFinite(arr[i])) { continuous = false; continue; }
      const x = PAD.l + (cw * i) / (MAX_POINTS - 1);
      const v = clamp(arr[i], 0, Y_MAX);
      const y = PAD.t + ch - (v / Y_MAX) * ch;
      if (continuous) ctx.lineTo(x, y);
      else ctx.moveTo(x, y);
      continuous = true;
    }
    // 两层描边代替 shadowBlur：外层同色低透明度做柔光，内层实线保持锐利。
    // 柔光从 4px/30% 收到 3px/15%、核心线 1.6 → 2：
    // 4px 的柔光会把高频抖动的曲线（CPU 负载）糊成一条毛边带，
    // 收窄后锯齿能看清，同时留一点辉光，不至于在深色卡片上显得单薄。
    // 1% L 用更细更淡的线（1px / 65%），避免和 FPS 抢视觉重心
    const isL = key === "fps1l";
    ctx.strokeStyle = colors[key];
    ctx.globalAlpha = isL ? 0.1 : 0.15;
    ctx.lineWidth = isL ? 1.5 : 3;
    ctx.stroke();

    ctx.globalAlpha = isL ? 0.65 : 1;
    ctx.lineWidth = isL ? 1 : 2;
    ctx.stroke();
  }
  ctx.globalAlpha = 1;
}

/* ---------------- data ---------------- */

function applyData(d) {
  setText("cpu-load", d.cpuLoad, 0);
  setRing($("cpu-ring"), d.cpuLoad);
  setText("cpu-temp", d.cpuTemp, 0);
  setText("cpu-clock", d.cpuClock, 2);
  setText("cpu-volt", d.cpuVolt, 3);
  setText("cpu-power", d.cpuPower, 0);

  setText("gpu-load", d.gpuLoad, 0);
  setRing($("gpu-ring"), d.gpuLoad);
  setText("gpu-temp", d.gpuTemp, 0);
  setText("gpu-clock", d.gpuClock, 2);
  setText("gpu-volt", d.gpuVolt, 3);
  setText("gpu-power", d.gpuPower, 0);

  setRaw("fps", d.fps);
  setRaw("resolution", d.res);
  setRaw("refresh", d.hz);

  // HWiNFO 那一行：FPS / 1% L / 帧时间
  setText("fps-hw", d.fps, 0);
  setText("fps1l-hw", d.fps1l, 0);
  setText("ft-hw", d.frame, 1);

  // 下载速度：固定 1 位小数，单位由 HTML 单独排版，位置不会跳
  const netText = Number.isFinite(Number(d.net)) ? Number(d.net).toFixed(1) : "0.0";
  const netEl = $("netval");
  if (netEl) netEl.textContent = netText;
  const netElHw = $("netval-hw");
  if (netElHw) netElHw.textContent = netText;

  // The snapshot provides only supported Windows battery readings.
  // 30 秒保鲜期内不覆盖 —— 那段时间显示的是接口的最后一次好数据。
  updateDevices(d.devices);

  // 风扇行与传感器的对应关系可能随机器变（换主板 / 换数据源），变了就重刷标题
  if (d.fanIds) {
    let idsChanged = false;
    for (let i = 0; i < 5; i++) {
      const id = d.fanIds[i];
      const label = d.fanLabels ? d.fanLabels[i] || "" : "";
      if (FAN_IDS[i] !== id || FAN_LABELS[i] !== label) {
        FAN_IDS[i] = id;
        FAN_LABELS[i] = label;
        idsChanged = true;
      }
    }
    if (idsChanged) renderFanNames();
  }

  const keys = ["cpu", "aio", "opt", "1", "2"];
  keys.forEach((k, i) => {
    const rpm = d.fans[i];
    const row = $(`fan-${k}`)?.closest(".fan-row");
    if (row && row.hidden !== (rpm == null)) row.hidden = rpm == null;
    setText(`fan-${k}`, rpm, 0);
    let pct;
    if (d.fanPct) pct = d.fanPct[i];
    else pct = rpm == null || rpm <= 0 ? 0 : clamp((rpm / CONFIG.fanMax) * 100, 4, 100);
    setBar(`fan-${k}-bar`, pct);
  });

  renderStorage(d.disks);

  setText("pkg-temp", d.pkg, 0);
  setArc($("pkg-arc"), d.pkg);
  setText("gpu-hot", d.hot, 0);
  // 右侧小卡：显存温度 / 热点温度，由属性 gpuhot 切换。
  // 两个值都缓存下来，属性一变就能立刻换数字，不用等下一拍数据。
  setText("gpu-mem", d.mem, 0);

  pushHistory({ fps: d.fps, cpu: d.cpuLoad, gpu: d.gpuLoad, mem: d.mem, fps1l: d.fps1l });
  drawChart();
}

/* 横屏右上角状态组件：弧线 = 第一个设备电量 / 下方 4 点 = 第二个设备电量 / WiFi 信号 */
let WIFI_BARS = 0;   // 上一次的信号格数，用来做滞回

function renderStatusBar(arcVal, dotsVal, wlan) {
  // 上方 240° 弧线：电池百分比
  const arc = $("vol-arc");
  if (arc) {
    const len = 54 * (240 * Math.PI / 180);         // 240° 弧长 ≈ 226.2
    const v = Math.max(0, Math.min(100, Number(arcVal) || 0));
    arc.style.strokeDasharray = `${(v / 100) * len} 400`;
  }

  // 下方 4 个点：第二个设备电量，每格 25%（设备休眠时值为 0，四点全灭）
  const b = Math.max(0, Math.min(100, Number(dotsVal) || 0));
  const batBars = b <= 0 ? 0 : Math.ceil(b / 25);
  document.querySelectorAll(".bat-dot").forEach((d, i) => {
    d.classList.toggle("on", i < batBars);
  });

  // WiFi 信号：来自 bridge 的 netsh（跟数据源无关）。
  // RSSI 会 ±3dBm 抖动，卡在边界时图标会反复闪，所以升格和降格用不同门槛：
  //   升到 3 格要 ≥ -58，但已经在 3 格时要跌到 -62 以下才掉
  const r = wlan && wlan.ok && Number.isFinite(Number(wlan.rssi)) ? Number(wlan.rssi) : null;
  if (r == null) {
    WIFI_BARS = 0;                                  // 没连上 / 没有无线网卡
  } else {
    const t3 = WIFI_BARS >= 3 ? -62 : -58;
    const t2 = WIFI_BARS >= 2 ? -80 : -76;
    WIFI_BARS = r >= t3 ? 3 : r >= t2 ? 2 : 1;
  }
  document.querySelectorAll(".wifi-sig [data-i]").forEach((el) => {
    el.classList.toggle("on", Number(el.dataset.i) < WIFI_BARS);
  });
}

/* 弧线的电池值：优先 AIDA64 的 SBATT。
 * 台式机没有电池时它返回 "No Battery"（numeric() 会把它过滤成 null），
 * 这时退回到设备电量里第一个已识别设备的百分比。 */
function pickArcBattery(sensors, firstDevPct) {
  const cell = sensors && sensors.SBATT;
  const raw = cell ? String(cell.value == null ? "" : cell.value).trim() : "";
  if (raw && !/no\s*batt/i.test(raw)) {
    const n = parseFloat(raw.replace(/[^0-9.]/g, ""));
    if (Number.isFinite(n)) return Math.max(0, Math.min(100, n));
  }
  return firstDevPct;
}

window.__testStatus = renderStatusBar;   // 调试用：手动喂数据

/* CPU 风扇和水泵优先，其余使用采集器的顺序，空槽在显示时隐藏。 */
function pickFans(sensors) {
  const order = [];
  for (const id in sensors) {
    const s = sensors[id];
    if (s && s.type === "fan") order.push(id);
  }
  const rows = [];
  const used = new Set();
  for (const id of ["FCPU", "FAIOPUMP"]) {
    if (sensors[id] && sensors[id].type === "fan") { rows.push(id); used.add(id); }
  }
  for (const id of order) {
    if (rows.length >= 5) break;
    if (used.has(id)) continue;
    rows.push(id); used.add(id);
  }
  while (rows.length < 5) rows.push(null);
  return rows;
}

function fromSensors(sensors, wlan, disks = []) {
  const cpuLoad = cpuLoadOf(sensors);
  const gpuLoad = pick(sensors, "SGPU1UTI");
  const mem = pick(sensors, "SMEMUTI");
  const cpuClock = pick(sensors, "SCPUCLK");
  const gpuClock = pick(sensors, "SGPU1CLK");
  const hz = pick(sensors, "SVREFRATE");
  const fanIds = pickFans(sensors);

  // 下载速度：取所有网卡 DLRATE 的最大值。
  // 不能按「SNIC{i}CONNSPD > 0 的第一张」来选 —— CONNSPD 是链路协商速率，
  // 不代表这张卡在用：虚拟网卡 / 蓝牙 PAN / Hyper-V vEthernet 也会有协商速率
  // （实测本机 NIC1 = 3 Mbps、DLRATE 恒 0，却因为编号靠前把真实网卡 NIC3
  // 的流量顶掉了，网速永远显示 0.0）。取最大值天然偏向真有流量的那张，
  // 空闲卡的 0 不会赢。
  let nic = 0;
  for (let i = 1; i <= 8; i++) {
    const n = numeric(sensors, `SNIC${i}DLRATE`);
    if (n != null && n > nic) nic = n;
  }

  // Windows peripheral battery snapshot
  const devices = buildDevices();

  // 状态组件：弧线走电池（SBATT，台式机没有则退回第一个设备电量），
  // 下方 4 个点走第二个设备电量。都按 EasyBluetooth 的默认输出顺序取，
  // 不看有没有值 —— 休眠设备就是 0，四点全灭。
  const dev1 = devices[0] || {};
  const dev2 = devices[1] || {};
  renderStatusBar(
    pickArcBattery(sensors, dev1.pct ?? null),
    dev2.pct ?? 0,
    wlan
  );

  return {
    cpuLoad,
    cpuTemp: pick(sensors, "TCPU", "TCPUPKG"),
    cpuClock: cpuClock == null ? null : cpuClock / 1000,
    cpuVolt: pick(sensors, "VCPU", "VCPUVID"),
    cpuPower: pick(sensors, "PCPUPKG"),
    gpuLoad,
    gpuTemp: pick(sensors, "TGPU1", "TGPU1HOT"),
    gpuClock: gpuClock == null ? null : gpuClock / 1000,
    gpuVolt: pick(sensors, "VGPU1"),
    gpuPower: pick(sensors, "PGPU1"),
    fps: round0(pick(sensors, "SRTSSFPS")),
    fps1l: round0(pick(sensors, "SRTSSFPS1L")),
    frame: pick(sensors, "SFRAMETIME"),
    res: raw(sensors, "SDESKRES") || `${screen.width} x ${screen.height}`,
    hz: hz == null ? null : `${Math.round(hz)} Hz`,
    net: nic / 1024,   // 只存数字，格式化交给 applyData（固定 1 位小数）
    devices,
    fans: fanIds.map((id) => (id ? pick(sensors, id) : null)),
    fanIds,
    fanLabels: fanIds.map((id) => {
      const s = id && sensors[id];
      return s ? String(s.label || "") : "";
    }),
    disks,
    // 温度圆环：3 封装（默认）/ 1 中央处理器(CPU) / 2 CPU 二极管，不做候选链
    pkg: TEMPRING === 1 ? pick(sensors, "TCPU")
      : TEMPRING === 2 ? pick(sensors, "TCPUDIO")
        : pick(sensors, "TCPUPKG"),
    // GPU 核心温度：两个数据源都取 TGPU1（AIDA64 的 GPU / HWiNFO 的 GPU Temperature）
    hot: pick(sensors, "TGPU1", "TGPU1HOT"),
    // GPU memory temperature.
    mem: pick(sensors, "TGPU1MEM"),
    spot: pick(sensors, "TGPU1HOT")
  };
}

/* 右下角的传感器链路提示：只反映 bridge / AIDA64，和设备区无关。
 * 只在文字或显隐真的变化时才碰 DOM。 */
function setSensorStatus(show, text) {
  const el = $("status");
  if (!el) return;
  const t = $("status-text");
  if (t && text && t.textContent !== text) t.textContent = text;
  if (el.classList.contains("on") !== !!show) el.classList.toggle("on", !!show);
}

async function tick() {
  try {
    const data = await window.readRustWallpaperSnapshot();
    window.panelWeather.setCity(data.weatherCity);
    if (CONFIG.cpuName === DEFAULT_CPU_NAME && data.cpuName) setUserName("cpu", data.cpuName.replace(/\s+\d+-Core Processor$/i, ""));
    if (CONFIG.gpuName === DEFAULT_GPU_NAME && data.gpuName) setUserName("gpu", data.gpuName);
    const gpuName = $("gpu-name");
    if (gpuName && data.gpuPaused) {
      put("gpu-name", "采集已暂停");
      gpuName.title = "游戏兼容模式";
    } else if (gpuName?.title === "游戏兼容模式") {
      renderNames();
    }
    DEVICE_API.reachable = data.batteryAvailable;
    DEVICE_API.list = data.batteries;
    applyData(fromSensors(data.sensors, undefined, data.volumes));
    if ($("fps-hw")) $("fps-hw").title = data.fpsApp || "等待前台游戏";
    setSensorStatus(data.privilegedReadingsMissing, data.privilegedReadingsMissing ? "CPU 温度 / 主板 RPM 需要管理员权限" : "");
  } catch (err) {
    DEVICE_API.reachable = false;
    DEVICE_API.list = [];
    const offline = /source offline/.test(String(err && err.message));
    applyData(fromSensors({}));
    setSensorStatus(true, offline ? "Rust 采集暂无读数" : "Rust 采集器未连接");
  }
}
async function loop() {
  await tick();
  window.setTimeout(loop, 1000);
}

function updateClock() {
  const now = new Date();
  const hh = String(now.getHours()).padStart(2, "0");
  const mm = String(now.getMinutes()).padStart(2, "0");
  $("clock").textContent = `${hh}:${mm}`;

  const week = ["日", "一", "二", "三", "四", "五", "六"][now.getDay()];
  const month = ["一", "二", "三", "四", "五", "六", "七", "八", "九", "十", "十一", "十二"][now.getMonth()];
  $("date").textContent = `${month}月${now.getDate()}日 星期${week}`;

  window.panelWeather.updateDaylight();
}

/* 时钟只显示 HH:MM，没必要每秒刷。
 * 但不能简单用 setInterval(fn, 60000) —— 它是从启动时刻起算的，
 * 比如 13:51:09 启动，就永远在 :09 触发，分钟跳变会晚最多 59 秒。
 * 这里每次算到「下一个整分钟」还差多少毫秒，误差 <5ms。 */
function scheduleClock() {
  const now = new Date();
  const delay = 60000 - (now.getSeconds() * 1000 + now.getMilliseconds());
  window.setTimeout(() => {
    updateClock();
    scheduleClock();
  }, delay);
}

function bindDesktopPanel() {
  const post = message => window.ipc.postMessage(JSON.stringify(message));
  const surfaces = [...document.querySelectorAll(".card, #desktop-app-frame, #desktop-game-frame, #dock")];
  let frame;
  const scheduleRegion = () => {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = null;
      const scale = stage.getBoundingClientRect().width / stage.offsetWidth;
      const cards = surfaces.flatMap(element => {
        const {left, top, right, bottom, width, height} = element.getBoundingClientRect();
        if (!width || !height) return [];
        const radius = parseFloat(getComputedStyle(element).borderTopLeftRadius) || 0;
        return [[left, top, right, bottom, radius * scale]];
      });
      post({action: "region", viewport: [innerWidth, innerHeight], cards});
    });
  };
  window.addEventListener("resize", scheduleRegion);
  const resize = new ResizeObserver(scheduleRegion);
  for (const surface of surfaces) resize.observe(surface);
  scheduleRegion();

  const clockCard = document.querySelector(".card-a");
  clockCard.title = "按住 Shift 拖动时钟可移动面板";
  let drag;
  clockCard.addEventListener("pointerdown", event => {
    if (!event.shiftKey || event.button !== 0) return;
    drag = {id: event.pointerId, x: event.screenX, y: event.screenY};
    clockCard.setPointerCapture(event.pointerId);
    event.preventDefault();
  });
  clockCard.addEventListener("pointerup", event => {
    if (drag?.id !== event.pointerId) return;
    post({action: "move", delta: [event.screenX - drag.x, event.screenY - drag.y]});
    drag = null;
  });
  clockCard.addEventListener("lostpointercapture", () => { drag = null; });
}

function boot() {
  if (window.wallpaperDashboardPanel) document.documentElement.classList.add("standalone-panel");
  const qs = new URLSearchParams(location.search);
  if (qs.get("cpu")) CONFIG.cpuName = qs.get("cpu");
  if (qs.get("gpu")) CONFIG.gpuName = qs.get("gpu");
  // Optional battery ring view.
  if (qs.get("devstyle") === "2") {
    DEVSTYLE = 2;
    document.body.classList.add("devring");
  }

  renderNames();
  // 字体就绪后重量一次：measureText 依赖真实字形宽度，否则省略号位置会偏
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(renderNames).catch(() => {});
  }
  renderRingCap();
  bindDeviceScroll($("device-list"));
  renderRing([]);   // 先把圆环占位符铺好，等第一帧数据再填
  renderFanNames();
  window.initConfiguredDock();
  window.panelWeather.start(qs.get("user"));
  updateClock();
  scheduleClock();
  fitStage();
  if (window.wallpaperDashboardPanel) bindDesktopPanel();
  resizeCanvas();
  window.addEventListener("resize", () => {
    fitStage();
    resizeCanvas();
  });
  if (window.ResizeObserver) new ResizeObserver(resizeCanvas).observe(canvas);

  if (DEMO_ON) {
    $("clock").textContent = "08:24";
    $("date").textContent = "十月29日 星期四";
    applyData(DEMO);
    return;
  }
  loop();
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", boot);
} else {
  boot();
}
