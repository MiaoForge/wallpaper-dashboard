"use strict";

window.panelWeather = (() => {
  const $ = id => document.getElementById(id);

  const WEATHER = {
    loc: null,
    sunrise: "",   // API 的日出日落，用来判断白天/晚上
    sunset: "",
    timer: null
  };

  const WEATHER_TTL = 30 * 60 * 1000;   // 正常周期：30 分钟刷一次
  const WEATHER_RETRY_MIN = 5 * 1000;   // 失败后第一次重试：5 秒
  const WEATHER_RETRY_MAX = 60 * 1000;  // 连续失败最长等 60 秒
  let weatherRetry = 0;
  let weatherRequest = 0;

  /* Six US AQI bands used for the badge. */
  function aqiLevel(idx) {
    if (idx == null) return null;
    if (idx <= 1) return ["优", "green"];
    if (idx === 2) return ["良", "green"];
    if (idx === 3) return ["轻度", "orange"];
    if (idx === 4) return ["中度", "orange"];
    if (idx === 5) return ["重度", "purple"];
    return ["严重", "purple"];
  }

  /* 紫外线按 WHO 分级 */
  function uvLevel(uv) {
    if (uv == null || !Number.isFinite(Number(uv))) return null;
    const v = Number(uv);
    if (v < 3) return ["低", "purple"];
    if (v < 6) return ["中等", "green"];
    if (v < 8) return ["高", "orange"];
    if (v < 11) return ["很高", "orange"];
    return ["极高", "purple"];
  }

  function weatherQuery(base, parameters) {
    const url = new URL(base);
    for (const [key, value] of Object.entries(parameters)) url.searchParams.set(key, value);
    return url;
  }

  async function weatherJson(url, options = {}) {
    const response = await fetch(url, {cache: "no-store", signal: AbortSignal.timeout(9000), ...options});
    if (!response.ok) throw new Error(`Weather HTTP ${response.status}`);
    return response.json();
  }

  async function locateWeatherCity(city) {
    if (!city) return weatherJson("/weather-location", {method: "POST"});
    const {results} = await weatherJson(weatherQuery("https://geocoding-api.open-meteo.com/v1/search", {
      name: city.replace(/市$/, ""), count: 5, language: "zh",
    }));
    if (!results?.length) throw new Error("找不到城市");
    const place = results[0];
    return {city: place.name, lat: place.latitude, lon: place.longitude, tz_id: place.timezone};
  }

  function isDaytime() {
    const toMin = (s) => {
      const m = String(s || "").match(/(\d{1,2}):(\d{2})\s*(AM|PM)?/i);
      if (!m) return null;
      let h = Number(m[1]);
      const ap = (m[3] || "").toUpperCase();
      if (ap === "PM" && h < 12) h += 12;
      if (ap === "AM" && h === 12) h = 0;
      return h * 60 + Number(m[2]);
    };
    const now = new Date();
    const cur = now.getHours() * 60 + now.getMinutes();
    const a = toMin(WEATHER.sunrise);
    const b = toMin(WEATHER.sunset);
    if (a != null && b != null) return cur >= a && cur < b;
    return cur >= 360 && cur < 1080;
  }

  let _slogan = "";
  function updateSlogan() {
    const el = $("wslogan");
    if (!el) return;
    const text = isDaytime() ? "good day" : "good night";
    if (text === _slogan) return;
    _slogan = text;
    el.textContent = text;
  }

  /* ---------------- 天气图标（按 condition.code 选） ---------------- */

  function weatherCondition(code) {
    if (code === 0) return ["晴", "sun"];
    if (code === 1 || code === 2) return ["晴间多云", "partly"];
    if (code === 3) return ["阴", "cloud"];
    if (code === 45 || code === 48) return ["雾", "fog"];
    if (code >= 95) return ["雷雨", "storm"];
    if (code >= 71 && code <= 86 && code !== 80 && code !== 81 && code !== 82) return ["雪", "snow"];
    if (code >= 51 && code <= 57) return ["毛毛雨", "rain"];
    if (code >= 80) return ["阵雨", "rain"];
    if (code >= 61) return ["雨", "rain"];
    return ["--", ""];
  }

  function setWeatherIcon(kind) {
    const el = $("wicon");
    el.hidden = !kind;
    if (kind) el.src = `icons/${kind}.svg`;
    else el.removeAttribute("src");
  }

  const SYNODIC = 29.530588853;                       // 朔望月长度（天）
  const KNOWN_NEW = Date.UTC(2000, 0, 6, 18, 14) / 86400000; // 已知新月时刻

  /* 返回 0~1：0 = 新月，0.25 = 上弦，0.5 = 满月，0.75 = 下弦 */
  function moonPhase(date) {
    const days = date.getTime() / 86400000;
    let p = ((days - KNOWN_NEW) % SYNODIC) / SYNODIC;
    if (p < 0) p += 1;
    return p;
  }

  /* 亮面路径：外圈走半圆，明暗界线走半短轴随相位收缩的椭圆。
     盈月亮面在右（第一段顺时针），亏月亮面在左（第一段逆时针）；
     椭圆弧的凸向由 cos 的符号决定，凸出去才是宽月相、凹回来是细月牙 */
  function moonLitPath(p) {
    const R = 42, cx = 50, cy = 50;
    const cos = Math.cos(2 * Math.PI * p);
    const rx = Math.abs(R * cos);
    const sweep1 = p < 0.5 ? 1 : 0;
    const sweep2 = p < 0.5 ? (cos > 0 ? 0 : 1) : (cos > 0 ? 1 : 0);
    return `M ${cx} ${cy - R} A ${R} ${R} 0 0 ${sweep1} ${cx} ${cy + R}` +
           ` A ${rx} ${R} 0 0 ${sweep2} ${cx} ${cy - R} Z`;
  }

  let _orbDay = null;
  let _orbPhase = -1;

  function updateOrb() {
    const sun = document.querySelector(".orb-sun");
    const moon = document.querySelector(".orb-moon");
    const halo = document.querySelector(".orb-halo");
    if (!sun || !moon) return;

    const day = isDaytime();
    const phase = day ? 0 : moonPhase(new Date());
    const key = day ? -1 : Math.round(phase * 500);
    if (day === _orbDay && key === _orbPhase) return;   // 没变化就不重绘
    _orbDay = day;
    _orbPhase = key;

    sun.style.display = day ? "" : "none";
    moon.style.display = day ? "none" : "";
    if (halo) halo.setAttribute("fill", day ? "url(#gHaloSun)" : "url(#gHaloMoon)");
    // 月相：亮面用真实月球纹理，暗面盖一层半透明黑（纹理仍可见，只是压暗）
    if (!day) {
      const d = moonLitPath(phase);
      const lit = $("moon-lit-mask");
      const hole = $("moon-dark-hole");
      if (lit) lit.setAttribute("d", d);
      if (hole) hole.setAttribute("d", d);
    }
  }

  function renderWeatherUser(user) {
    const el = $("wuser");
    if (el) el.textContent = String(user ?? "").trim();
  }

  /* 地址只保留城市："四川省成都市" -> "成都市"，"合肥市新城区" -> "合肥市" */
  function cityOnly(addr) {
    const raw = String(addr || "").trim();
    if (!raw) return "";
    // 先去掉省级前缀（省 / 自治区 / 特别行政区）
    const rest = raw.replace(/^.*?(省|自治区|特别行政区)/, "").trim();
    const i = rest.indexOf("市");
    if (i > 0) return rest.slice(0, i + 1);
    // 没有"市"（直辖市以外的写法、或英文城市）就原样返回
    return rest || raw;
  }

  /* 时间卡片上的地址，只显示到"市" */
  function renderWeatherPlace(resolved = "") {
    const el = $("wloc");
    if (el) el.textContent = cityOnly(WEATHER.loc) || resolved || "--";
  }

  function roundWeather(value) {
    return Number.isFinite(value) ? Math.round(value) : "--";
  }

  function weatherBadge(id, level) {
    const el = $(id);
    el.textContent = level?.[0] ?? "--";
    el.className = "pill " + (level?.[1] ?? "");
  }

  function renderWeather({current = {}, daily = {}} = {}) {
    const round = roundWeather;
    const put = (id, value) => { $(id).textContent = value; };
    const [condition, icon] = weatherCondition(current.weather_code);
    put("wtemp", round(current.temperature_2m));
    put("wfeels", round(current.apparent_temperature));
    put("whum", round(current.relative_humidity_2m));
    put("whaze", condition);
    setWeatherIcon(icon);
    put("wmin", round(daily.temperature_2m_min?.[0]));
    put("wmax", round(daily.temperature_2m_max?.[0]));
    put("wwind", round(current.wind_speed_10m));
    put("wwindmax", round(daily.wind_speed_10m_max?.[0]));
    const rain = daily.precipitation_probability_max?.[0];
    put("wrain", round(rain));
    $("wrain").classList.toggle("blue", rain >= 60);
    WEATHER.sunrise = daily.sunrise?.[0]?.split("T")[1] ?? "";
    WEATHER.sunset = daily.sunset?.[0]?.split("T")[1] ?? "";
    put("wsunrise", WEATHER.sunrise || "--");
    put("wsunset", WEATHER.sunset || "--");
    updateDaylight();
    weatherBadge("wuv-pill", uvLevel(daily.uv_index_max?.[0]));
  }

  function renderAirQuality(airQuality) {
    $("waqi").textContent = roundWeather(airQuality);
    $("waqi").classList.toggle("orange", airQuality > 100);
    const limits = [50, 100, 150, 200, 300];
    const index = Number.isFinite(airQuality) ? (limits.findIndex(limit => airQuality <= limit) + 1 || 6) : null;
    weatherBadge("waqi-pill", aqiLevel(index));
  }

  function locationMatchesSystemOffset(timeZone) {
    if (!timeZone) return true;
    try {
      const now = new Date();
      const clock = new Intl.DateTimeFormat("en-GB", {
        timeZone, hour: "2-digit", minute: "2-digit", hourCycle: "h23",
      }).formatToParts(now);
      const hour = Number(clock.find(part => part.type === "hour")?.value);
      const minute = Number(clock.find(part => part.type === "minute")?.value);
      if (!Number.isFinite(hour) || !Number.isFinite(minute)) return true;
      const difference = Math.abs((now.getHours() - hour) * 60 + now.getMinutes() - minute);
      return Math.min(difference, 1440 - difference) <= 120;
    } catch { return true; }
  }

  async function refreshWeather() {
    if (WEATHER.loc === null) return;
    clearTimeout(WEATHER.timer);
    const version = ++weatherRequest;
    const city = WEATHER.loc;
    try {
      const place = await locateWeatherCity(city);
      if (version !== weatherRequest) return;
      if (!city && !locationMatchesSystemOffset(place.tz_id)) {
        renderWeatherPlace("定位偏差，请选城市");
        scheduleWeather(WEATHER_TTL);
        return;
      }
      renderWeatherPlace(place.city);
      const coordinates = {latitude: place.lat, longitude: place.lon, timezone: "auto"};
      const forecast = weatherJson(weatherQuery("https://api.open-meteo.com/v1/forecast", {
        ...coordinates, forecast_days: 1, wind_speed_unit: "ms",
        current: "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m",
        daily: "temperature_2m_min,temperature_2m_max,precipitation_probability_max,sunrise,sunset,wind_speed_10m_max,uv_index_max",
      })).then(data => {
        if (!data.current || !data.daily) throw new Error("Missing weather forecast");
        if (version === weatherRequest) renderWeather(data);
      }).catch(error => {
        if (version === weatherRequest) $("whaze").textContent = "天气更新失败，重试中";
        throw error;
      });
      const air = weatherJson(weatherQuery("https://air-quality-api.open-meteo.com/v1/air-quality", {
        ...coordinates, current: "us_aqi", forecast_days: 1,
      })).then(data => {
        const index = data.current?.us_aqi;
        if (version === weatherRequest) renderAirQuality(index);
      });
      const results = await Promise.allSettled([forecast, air]);
      if (version !== weatherRequest) return;
      if (results.every(result => result.status === "fulfilled")) {
        weatherRetry = 0;
        scheduleWeather(WEATHER_TTL);
        return;
      }
    } catch {
      if (version !== weatherRequest) return;
      if (!city) renderWeatherPlace("自动定位暂不可用");
      $("whaze").textContent = "城市定位失败，重试中";
    }
    weatherRetry++;
    scheduleWeather(Math.min(WEATHER_RETRY_MAX, WEATHER_RETRY_MIN * (2 ** (weatherRetry - 1))));
  }

  function scheduleWeather(delay) {
    if (WEATHER.timer) clearTimeout(WEATHER.timer);
    WEATHER.timer = setTimeout(refreshWeather, delay);
  }

  function start(user) {
    renderWeatherUser(user);
    renderWeatherPlace();
    window.addEventListener("online", () => {
      weatherRetry = 0;
      refreshWeather();
    });
  }

  function setCity(city) {
    if (typeof city !== "string" || city === WEATHER.loc) return;
    WEATHER.loc = city;
    weatherRetry = 0;
    renderWeatherPlace();
    renderWeather();
    renderAirQuality();
    refreshWeather();
  }

  function updateDaylight() {
    updateSlogan();
    updateOrb();
  }

  return {start, setCity, updateDaylight};
})();
