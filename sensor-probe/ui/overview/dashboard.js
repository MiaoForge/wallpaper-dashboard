"use strict";

const endpoint = "/snapshots";
const labels = {
  windows: "Windows", amd_adlx: "AMD 显卡", amd_ryzen_sdk: "AMD CPU",
  msi_center: "MSI 主板", nvidia_nvml: "NVIDIA 显卡",
};
const statusText = {
  available: "在线", unavailable: "暂无数据", unsupported: "不适用",
  permission_denied: "权限不足", stale: "数据过期",
};
const freshness = { amd_ryzen_sdk: 12000 };
const get = id => document.getElementById(id);
const settings = get("device-settings");
const select = get("gpu-select");
const fanNames = get("fan-names");
const history = [];
let snapshot = [];
let availableGpus = [];
let availableFans = [];
let previewGpuKey = null;
let optionsSignature = "";
let prefs = { gpu: "", fans: {} };
try {
  const saved = JSON.parse(localStorage.getItem("dashboard.devices.v1") || "{}");
  if (typeof saved.gpu === "string") prefs.gpu = saved.gpu;
  if (saved.fans && typeof saved.fans === "object") prefs.fans = saved.fans;
} catch { /* Rendering still works without storage. */ }

function save() {
  try { localStorage.setItem("dashboard.devices.v1", JSON.stringify(prefs)); } catch { /* Optional storage. */ }
}

const background = get("background");
let previewImage = "";
function showBackground() {
  const image = previewImage;
  background.hidden = !image;
  if (image) background.src = image;
  else background.removeAttribute("src");
}
get("background-file").addEventListener("change", event => {
  if (previewImage) URL.revokeObjectURL(previewImage);
  const file = event.target.files[0];
  previewImage = file ? URL.createObjectURL(file) : "";
  showBackground();
});

function clock() {
  const now = new Date();
  get("clock").textContent = now.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
  get("date").textContent = now.toLocaleDateString("zh-CN", { month: "long", day: "numeric", weekday: "long" });
}
clock();
setInterval(clock, 1000);

function backendStatus(backend) {
  const age = Date.now() - backend.sampled_at_unix_ms;
  if (!Number.isFinite(age) || age < -2000 || age > (freshness[backend.source] ?? 7000)) return "stale";
  return backend.status;
}

function devices(kind) {
  return snapshot.flatMap(backend => backend.devices
    .filter(device => device.kind === kind)
    .map(device => ({ ...device, backend, key: JSON.stringify([backend.source, device.id]) })));
}

function retainedReading(backend) {
  if (!backend?.last_error || backendStatus(backend) !== "available") return "";
  const seconds = Math.max(0, Math.floor((Date.now() - backend.sampled_at_unix_ms) / 1000));
  return "上次读数 · " + seconds + " 秒前";
}

function sensor(device, id) {
  if (!device || backendStatus(device.backend) !== "available") return undefined;
  return device.sensors.find(item => item.id === id && item.status === "available" && Number.isFinite(item.value));
}

function format(item, digits = 0) {
  if (!item) return "—";
  return item.unit === "B"
    ? (item.value / 1073741824).toFixed(1) + " GiB"
    : item.value.toFixed(digits) + " " + item.unit;
}

function rows(id, values) {
  get(id).replaceChildren(...values.map(([label, value, ratio]) => {
    const row = document.createElement("div");
    const name = document.createElement("dt");
    const reading = document.createElement("dd");
    name.textContent = label;
    reading.textContent = value;
    row.append(name, reading);
    if (Number.isFinite(ratio)) {
      const meter = document.createElement("meter");
      meter.max = 1;
      meter.value = Math.max(0, Math.min(1, ratio));
      meter.setAttribute("aria-label", label);
      row.append(meter);
    }
    return row;
  }));
}

function savedFanLabel(key) {
  return Object.prototype.hasOwnProperty.call(prefs.fans, key) && typeof prefs.fans[key] === "string" ? prefs.fans[key] : "";
}

function syncSettings() {
  const signature = JSON.stringify([prefs.gpu, availableGpus.map(({ key, name }) => [key, name])]);
  if (signature === optionsSignature) return;
  select.replaceChildren(new Option("自动选择", ""));
  for (const gpu of availableGpus) select.add(new Option(gpu.name, gpu.key));
  if (prefs.gpu && !availableGpus.some(gpu => gpu.key === prefs.gpu)) {
    select.add(new Option("已选显卡暂不可用", prefs.gpu));
  }
  select.value = prefs.gpu;
  optionsSignature = signature;
}

get("settings").addEventListener("click", () => {
  syncSettings();
  fanNames.replaceChildren(...availableFans.map(({ key, item }) => {
    const label = document.createElement("label");
    label.textContent = item.label;
    const input = document.createElement("input");
    input.value = savedFanLabel(key);
    input.placeholder = item.label;
    input.maxLength = 40;
    input.addEventListener("input", () => {
      const name = input.value.trim();
      if (name) prefs.fans[key] = name;
      else delete prefs.fans[key];
      save();
    });
    label.append(input);
    return label;
  }));
  if (!availableFans.length) fanNames.textContent = "暂无可配置的风扇";
  settings.showModal();
});
select.addEventListener("change", () => { prefs.gpu = select.value; save(); render(false); });
settings.addEventListener("close", () => render(false));

function render(recordHistory = true) {
  const bySource = source => snapshot.find(backend => backend.source === source);
  const cpu = devices("cpu").find(device => device.backend.source === "windows");
  const cpuTelemetry = devices("cpu").find(device => device.backend.source === "amd_ryzen_sdk");
  const ram = devices("memory")[0];
  availableGpus = devices("gpu");
  const ranked = [...availableGpus].sort((a, b) => (sensor(b, "memory_total")?.value ?? 0) - (sensor(a, "memory_total")?.value ?? 0));
  const gpu = prefs.gpu ? availableGpus.find(item => item.key === prefs.gpu) : ranked.find(item => backendStatus(item.backend) === "available");
  const gpuKey = gpu?.key ?? null;
  if (gpuKey !== previewGpuKey) {
    for (const sample of history) sample[1] = undefined;
    previewGpuKey = gpuKey;
  }
  const cpuUse = sensor(cpu, "usage");
  const gpuUse = sensor(gpu, "usage");
  get("cpu-name").textContent = cpu?.name ?? "—";
  get("gpu-name").textContent = gpu?.name ?? (prefs.gpu ? "已选显卡暂不可用" : "—");
  get("cpu-use").textContent = format(cpuUse);
  get("cpu-notice").textContent = retainedReading(bySource("amd_ryzen_sdk"));
  get("fan-notice").textContent = retainedReading(bySource("msi_center"));
  get("gpu-use").textContent = format(gpuUse);
  get("cpu-gauge").style.setProperty("--fill", Math.min(100, Math.max(0, cpuUse?.value ?? 0)) + "%");
  get("gpu-gauge").style.setProperty("--fill", Math.min(100, Math.max(0, gpuUse?.value ?? 0)) + "%");
  rows("temperatures", [
    ["CPU", format(sensor(cpuTelemetry, "temperature"), 1)],
    ["GPU 核心", format(sensor(gpu, "core_temperature"))],
    ["显存", format(sensor(gpu, "memory_temperature"))],
  ]);
  rows("cpu-metrics", [
    ["温度", format(sensor(cpuTelemetry, "temperature"), 1)],
    ["插槽功耗", format(sensor(cpuTelemetry, "socket_power"), 1)],
    ["核心 0 频率", format(sensor(cpuTelemetry, "core:0:clock"))],
  ]);
  rows("gpu-metrics", [
    ["核心温度", format(sensor(gpu, "core_temperature"))],
    ["显存温度", format(sensor(gpu, "memory_temperature"))],
    ["整卡功耗", format(sensor(gpu, "board_power"), 1)],
  ]);
  const fanDevices = devices("motherboard");
  if (fanDevices.length) {
    availableFans = fanDevices.flatMap(device => device.sensors.filter(item => item.unit === "RPM")
      .map(item => ({ key: JSON.stringify([device.key, item.id]), device, item })));
  }
  rows("fans", availableFans.length ? availableFans.map(({ key, device, item }) => {
    const current = fanDevices.find(fan => fan.key === device.key);
    const reading = sensor(current, item.id);
    return [savedFanLabel(key) || item.label, format(reading), reading ? reading.value / 4000 : undefined];
  }) : [["主板转速", statusText[bySource("msi_center") ? backendStatus(bySource("msi_center")) : "unavailable"]]]);
  rows("storage", [
    ["内存占用", format(sensor(ram, "used")), sensor(ram, "usage")?.value / 100],
    ...devices("volume").map(volume => {
      const free = sensor(volume, "free");
      const total = sensor(volume, "total");
      return [volume.name + " 可用", format(free), 1 - free?.value / total?.value];
    }),
  ]);
  get("sources").replaceChildren(...Object.entries(labels).map(([source, label]) => {
    const backend = bySource(source);
    const li = document.createElement("li");
    li.textContent = label + " · " + (backend ? statusText[backendStatus(backend)] ?? "暂无数据" : "暂无数据");
    const retained = retainedReading(backend);
    if (retained) li.textContent = label + " · " + retained;
    if (backend?.last_error?.detail || backend?.detail) li.title = backend?.last_error?.detail ?? backend.detail;
    return li;
  }));
  if (settings.open) syncSettings();
  if (recordHistory) {
    history.push([cpuUse?.value, gpuUse?.value, sensor(ram, "usage")?.value]);
    if (history.length > 60) history.shift();
  }
  chart();
}

function chart() {
  const canvas = get("history");
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (!width || !height) return;
  const dpi = Math.min(devicePixelRatio || 1, 2);
  canvas.width = Math.round(width * dpi);
  canvas.height = Math.round(height * dpi);
  const context = canvas.getContext("2d");
  if (!context) return;
  context.scale(dpi, dpi);
  const left = 40;
  const inner = height - 16;
  context.font = "11px system-ui";
  context.textBaseline = "middle";
  for (const level of [0, 25, 50, 75, 100]) {
    const y = 8 + inner * (1 - level / 100);
    context.fillStyle = "#b3b9d0";
    context.fillText(level + "%", 0, y);
    context.strokeStyle = "#e0e5ff25";
    context.beginPath();
    context.moveTo(left, y);
    context.lineTo(width, y);
    context.stroke();
  }
  ["#8cecff", "#b6a3ff", "#ffd998"].forEach((color, series) => {
    context.strokeStyle = color;
    context.lineWidth = 2;
    context.beginPath();
    let previous = false;
    history.forEach((sample, index) => {
      const value = sample[series];
      if (!Number.isFinite(value)) { previous = false; return; }
      const x = width - (history.length - 1 - index) * (width - left) / 59;
      const y = 8 + inner * (1 - Math.max(0, Math.min(100, value)) / 100);
      if (previous) context.lineTo(x, y);
      else context.moveTo(x, y);
      previous = true;
    });
    context.stroke();
  });
}
window.addEventListener("resize", chart);

async function poll() {
  const status = get("connection");
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 2800);
  try {
    const response = await fetch(endpoint, { cache: "no-store", signal: controller.signal });
    if (!response.ok) throw new Error("HTTP " + response.status);
    const payload = await response.json();
    if (payload.schema_version !== 1 || !Array.isArray(payload.backends)) throw new Error("Invalid snapshot");
    snapshot = payload.backends;
    status.textContent = snapshot.some(backend => backendStatus(backend) === "available" && !backend.last_error) ? "实时采集" : "等待可用读数";
    status.classList.toggle("live", status.textContent === "实时采集");
  } catch {
    snapshot = [];
    status.textContent = "采集服务未连接";
    status.classList.remove("live");
  } finally {
    clearTimeout(timeout);
    render();
    setTimeout(poll, 1000);
  }
}
poll();
