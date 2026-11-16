"use strict";

window.readRustWallpaperSnapshot = async function () {
  const controller = new AbortController();
  const deadline = setTimeout(() => controller.abort(), 2500);
  try {
    const response = await fetch("http://127.0.0.1:8766/snapshots", {
      cache: "no-store",
      signal: controller.signal,
    });
    if (!response.ok) throw new Error("HTTP " + response.status);
    const snapshot = await response.json();
    if (snapshot.schema_version !== 1 || !Array.isArray(snapshot.backends)) {
      throw new Error("unsupported snapshot");
    }
    const batterySources = ["windows_battery", "bluetooth_devices", "hid_battery"];
    const bySource = source => snapshot.backends.find(backend =>
      backend.source === source && backend.status === "available" &&
      Math.abs(Date.now() - backend.sampled_at_unix_ms) <= (batterySources.includes(source) ? 45000 : source === "amd_ryzen_sdk" ? 12000 : 7000));
    const windows = bySource("windows");
    const cpu = windows?.devices.find(device => device.kind === "cpu");
    const cpuMetrics = bySource("amd_ryzen_sdk")?.devices.find(device => device.kind === "cpu");
    const gpu = (snapshot.gpu_paused ? [] : ["amd_adlx", "nvidia_nvml", "windows_gpu"]).flatMap(source => bySource(source)?.devices ?? [])
      .filter(device => device.kind === "gpu")
      .sort((a, b) =>
        ((reading(b, "memory_total") ?? 0) - (reading(a, "memory_total") ?? 0)) ||
        Number(["usage", "main_temperature"].some(id => reading(b, id) !== undefined)) -
        Number(["usage", "main_temperature"].some(id => reading(a, id) !== undefined)))[0];
    const memory = windows?.devices.find(device => device.kind === "memory");
    const frames = bySource("presentmon")?.devices.find(device => device.kind === "frames");
    const batteries = batterySources.map(bySource).filter(Boolean);
    const board = bySource("msi_center")?.devices.find(device => device.kind === "motherboard");
    const sensors = {};
    const put = (id, value, type = "sys", label = id) => {
      if (Number.isFinite(value)) sensors[id] = { type, label, value };
    };
    for (const [id, device, field] of [
      ["SCPUUTI", cpu, "usage"], ["TCPUPKG", cpuMetrics, "temperature"],
      ["TCPU", cpuMetrics, "temperature"], ["SCPUCLK", cpuMetrics, "core:0:clock"],
      ["VCPU", cpuMetrics, "telemetry_voltage"], ["PCPUPKG", cpuMetrics, "socket_power"],
      ["SGPU1UTI", gpu, "usage"], ["SGPU1CLK", gpu, "core_clock"],
      ["VGPU1", gpu, "core_voltage"], ["PGPU1", gpu, "board_power"],
      ["TGPU1", gpu, "core_temperature"], ["TGPU1", gpu, "main_temperature"],
      ["TGPU1HOT", gpu, "hotspot_temperature"],
      ["TGPU1MEM", gpu, "memory_temperature"], ["SMEMUTI", memory, "usage"],
      ["SRTSSFPS", frames, "fps"], ["SRTSSFPS1L", frames, "fps_1_percent_low"], ["SFRAMETIME", frames, "frame_time"],
    ]) put(id, reading(device, field));
    const rates = (windows?.devices ?? []).filter(device => device.kind === "network")
      .map(device => reading(device, "receive_rate")).filter(Number.isFinite);
    if (rates.length) put("SNIC1DLRATE", Math.max(...rates) / 1024);
    const units = 1024 ** 3;
    const volumes = (windows?.devices ?? []).filter(device => device.kind === "volume").flatMap(volume => {
      const letter = /^[A-Z]:/i.exec(volume.name)?.[0].toUpperCase();
      const free = reading(volume, "free");
      const total = reading(volume, "total");
      if (!letter || !Number.isFinite(free) || !(total > 0)) return [];
      const used = Math.max(0, total - free);
      return [{ letter, name: volume.volume_label ? `${volume.volume_label} (${letter})` : letter,
        pct: used * 100 / total, size: `${Math.round(used / units)} / ${Math.round(total / units)} GiB` }];
    }).sort((a, b) => a.letter.localeCompare(b.letter));
    for (const fan of board?.sensors ?? []) {
      if (fan.status !== "available" || fan.unit !== "RPM") continue;
      if (!(fan.value > 0 || fan.seen_spinning === true)) continue;
      const id = /^CPU Fan 1$/i.test(fan.label) ? "FCPU"
        : /^PUMP Fan 1$/i.test(fan.label) ? "FAIOPUMP" : fan.id;
      put(id, fan.value, "fan", fan.label);
    }
    if (!Object.keys(sensors).length) throw new Error("source offline");
    return { sensors, cpuName: cpu?.name, gpuName: gpu?.name, fpsApp: frames?.name,
      weatherCity: snapshot.weather_city, gpuPaused: snapshot.gpu_paused === true, volumes,
      batteryAvailable: batteries.length > 0,
      batteries: batteries.flatMap(backend => backend.devices).filter(device => ["battery", "gamepad", "peripheral"].includes(device.kind)).map(device => ({
        name: device.name, pct: reading(device, "charge"), charging: reading(device, "external_power") === 1, connection: device.connection, type: device.kind === "gamepad" ? "gamepad" : "",
      })),
      privilegedReadingsMissing: snapshot.backends.some(backend => ["amd_ryzen_sdk", "msi_center"].includes(backend.source) && backend.status === "permission_denied") };
  } finally {
    clearTimeout(deadline);
  }
};

function reading(device, id) {
  const entry = device?.sensors.find(sensor => sensor.id === id && sensor.status === "available");
  return Number.isFinite(entry?.value) ? entry.value : undefined;
}
