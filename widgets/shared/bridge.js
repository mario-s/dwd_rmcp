// DWD widget bridge — minimal MCP Apps (ui://, text/html;profile=mcp-app) client.
// Injected into every widget by the Rust server at the DWD:SHARED marker.
// Widgets only use window.DWD; they never talk postMessage directly.
(function () {
  "use strict";

  const PROTOCOL_VERSION = "2025-11-21";
  const inIframe = window.parent && window.parent !== window;
  const pending = new Map();
  const dataCbs = [];
  const inputCbs = [];
  const contextCbs = [];
  let nextId = 1;
  let lastData = null;
  let lastResult = null;

  function send(msg) {
    if (inIframe) window.parent.postMessage(msg, "*");
  }

  function request(method, params) {
    if (!inIframe) return Promise.reject(new Error("Not running inside an MCP Apps host"));
    return new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, { resolve, reject });
      send({ jsonrpc: "2.0", id, method, params: params || {} });
    });
  }

  function notify(method, params) {
    send({ jsonrpc: "2.0", method, params: params || {} });
  }

  function extractData(result) {
    if (!result) return null;
    if (result.structuredContent) return result.structuredContent;
    // Fallback: first text block containing JSON.
    const text = (result.content || []).find((c) => c.type === "text");
    if (text) {
      try { return JSON.parse(text.text); } catch (_) { /* not JSON */ }
    }
    return null;
  }

  function emitData(data, result) {
    if (!data) return;
    lastData = data;
    lastResult = result || null;
    document.documentElement.dataset.state = "ready";
    dataCbs.forEach((cb) => { try { cb(data, result); } catch (e) { console.error(e); } });
    scheduleSize();
  }

  function applyContext(ctx) {
    if (!ctx) return;
    DWD.context = Object.assign({}, DWD.context, ctx);
    if (ctx.theme) {
      DWD.theme = ctx.theme;
      document.documentElement.dataset.theme = ctx.theme;
    }
    const vars = ctx.styles && ctx.styles.variables;
    if (vars) {
      for (const [k, v] of Object.entries(vars)) {
        if (v != null) document.documentElement.style.setProperty(k, v);
      }
    }
    contextCbs.forEach((cb) => { try { cb(DWD.context); } catch (e) { console.error(e); } });
  }

  window.addEventListener("message", (event) => {
    const m = event.data;
    if (!m || m.jsonrpc !== "2.0") return;

    // Response to one of our requests.
    if (m.id != null && !m.method) {
      const p = pending.get(m.id);
      if (!p) return;
      pending.delete(m.id);
      if (m.error) p.reject(Object.assign(new Error(m.error.message || "MCP error"), m.error));
      else p.resolve(m.result);
      return;
    }

    switch (m.method) {
      case "ui/notifications/tool-input":
        inputCbs.forEach((cb) => cb((m.params && m.params.arguments) || {}));
        break;
      case "ui/notifications/tool-result":
        emitData(extractData(m.params), m.params);
        break;
      case "ui/notifications/host-context-changed":
        applyContext(m.params);
        break;
      case "ui/resource-teardown":
      case "ping":
        if (m.id != null) send({ jsonrpc: "2.0", id: m.id, result: {} });
        break;
      default:
        if (m.id != null) {
          send({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "Method not found" } });
        }
    }
  });

  // ---- auto-size --------------------------------------------------------
  let sizeQueued = false;
  let lastHeight = 0;
  function scheduleSize() {
    if (sizeQueued) return;
    sizeQueued = true;
    requestAnimationFrame(() => {
      sizeQueued = false;
      const h = Math.ceil(document.documentElement.getBoundingClientRect().height);
      if (h !== lastHeight) {
        lastHeight = h;
        notify("ui/notifications/size-changed", { width: document.documentElement.scrollWidth, height: h });
      }
    });
  }
  if (typeof ResizeObserver !== "undefined") {
    new ResizeObserver(scheduleSize).observe(document.documentElement);
  }

  // ---- formatting helpers ----------------------------------------------
  const LOCALE = "de-DE";
  const TZ = "Europe/Berlin";
  const nf = (d) => new Intl.NumberFormat(LOCALE, { minimumFractionDigits: d, maximumFractionDigits: d });
  const isNum = (v) => typeof v === "number" && isFinite(v);

  const DIRS = ["N", "NO", "O", "SO", "S", "SW", "W", "NW"];

  // WMO 4677 present-weather code (ICON "WW") → German label + icon key.
  // Local (Europe/Berlin) hour 0–23 as a number.
  function localHour(iso) {
    if (!iso) return null;
    const part = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", hourCycle: "h23", timeZone: TZ })
      .formatToParts(new Date(iso)).find((p) => p.type === "hour");
    return part ? Number(part.value) : null;
  }
  // Coarse day/night split for icons (no solar geometry needed at this scale).
  function isNight(iso) {
    const h = localHour(iso);
    return h != null && (h < 7 || h >= 19);
  }

  function wwInfo(code, iso) {
    const c = isNum(code) ? Math.round(code) : null;
    if (c == null) return { label: "–", icon: null };
    const night = isNight(iso);
    if (c >= 95) return { label: c >= 96 ? "Gewitter mit Hagel" : "Gewitter", icon: "thunder" };
    if (c >= 85) return { label: "Schneeschauer", icon: "snow" };
    if (c >= 80) return { label: c === 82 ? "Starke Regenschauer" : "Regenschauer", icon: "showers" };
    if (c >= 71) return { label: c >= 75 ? "Starker Schneefall" : "Schneefall", icon: "snow" };
    if (c === 66 || c === 67) return { label: "Gefrierender Regen", icon: "sleet" };
    if (c >= 61) return { label: c >= 65 ? "Starker Regen" : c >= 63 ? "Mäßiger Regen" : "Leichter Regen", icon: "rain" };
    if (c === 56 || c === 57) return { label: "Gefrierender Sprühregen", icon: "sleet" };
    if (c >= 51) return { label: "Sprühregen", icon: "drizzle" };
    if (c === 45 || c === 48) return { label: c === 48 ? "Reifnebel" : "Nebel", icon: "fog" };
    if (c === 3) return { label: "Bedeckt", icon: "cloud" };
    if (c === 2) return { label: "Bewölkt", icon: night ? "cloud" : "partly" };
    if (c === 1) return { label: "Leicht bewölkt", icon: night ? "moon" : "partly" };
    return { label: "Klar", icon: night ? "moon" : "sun" };
  }

  const S = 'fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"';
  const CLOUD = '<path d="M7 18h10a4 4 0 0 0 .6-7.96A5.5 5.5 0 0 0 7.1 9.2 4.4 4.4 0 0 0 7 18z"/>';
  const ICONS = {
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2.5v2.2M12 19.3v2.2M4.6 4.6l1.6 1.6M17.8 17.8l1.6 1.6M2.5 12h2.2M19.3 12h2.2M4.6 19.4l1.6-1.6M17.8 6.2l1.6-1.6"/>',
    moon: '<path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z"/>',
    partly: '<path d="M8.5 4.2v1.4M3.6 9.2H5M4.9 5.6l1 1M12.1 5.6l-1 1"/><path d="M5.6 11.4A3.4 3.4 0 0 1 11.6 8"/><path d="M9 20h8.5a3.6 3.6 0 0 0 .5-7.17A4.9 4.9 0 0 0 8.7 12a3.9 3.9 0 0 0 .3 8z"/>',
    cloud: CLOUD,
    fog: '<path d="M7 13h10a4 4 0 0 0 .6-7.96A5.5 5.5 0 0 0 7.1 4.2"/><path d="M3 16h18M5 19.5h14"/>',
    drizzle: '<g transform="translate(0,-3)">' + CLOUD + '</g><path d="M9 18.5v.5M12 19.5v.5M15 18.5v.5"/>',
    rain: '<g transform="translate(0,-3)">' + CLOUD + '</g><path d="M9 17.5l-1 3M13 17.5l-1 3M17 17.5l-1 3"/>',
    showers: '<g transform="translate(0,-3)">' + CLOUD + '</g><path d="M8.5 17.5l-1.5 4M12.5 17.5l-1.5 4M16.5 17.5l-1.5 4"/>',
    sleet: '<g transform="translate(0,-3)">' + CLOUD + '</g><path d="M9 17.5l-1 3"/><path d="M14.5 19h2M15.5 18v2"/>',
    snow: '<g transform="translate(0,-3)">' + CLOUD + '</g><path d="M8 19h2M9 18v2M14 19h2M15 18v2"/>',
    thunder: '<g transform="translate(0,-3)">' + CLOUD + '</g><path d="M12.5 15.5l-2 3.5h3l-2 3.5"/>',
    wind: '<path d="M3 8h11a2.5 2.5 0 1 0-2.5-2.5M3 12h16a2.5 2.5 0 1 1-2.5 2.5M3 16h8"/>',
    drop: '<path d="M12 3.5s6 6.4 6 10.5a6 6 0 0 1-12 0c0-4.1 6-10.5 6-10.5z"/>',
    gauge: '<path d="M4.5 17a8 8 0 1 1 15 0"/><path d="M12 13l3.5-4"/>',
    thermo: '<path d="M10 14.5V5a2 2 0 1 1 4 0v9.5a4 4 0 1 1-4 0z"/>',
    warn: '<path d="M12 3.5l9.5 16.5h-19z"/><path d="M12 10v4.5M12 17.2v.3"/>',
    bolt: '<path d="M13 2.5L5 13.5h6l-1 8 8-11h-6z"/>',
    layers: '<path d="M12 3l9 5-9 5-9-5z"/><path d="M3 13l9 5 9-5"/>',
    pin: '<path d="M12 21s-6.5-5.6-6.5-11a6.5 6.5 0 0 1 13 0C18.5 15.4 12 21 12 21z"/><circle cx="12" cy="10" r="2.3"/>',
    clock: '<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>',
    chat: '<path d="M4 5h16v11H9l-5 4z"/>',
    play: '<path d="M8 5.5v13l10-6.5z" fill="currentColor"/>',
    pause: '<path d="M8 5v14M16 5v14"/>',
  };

  function icon(name, size) {
    const s = size || 20;
    if (!name) return '<svg class="dwd-icon" width="' + s + '" height="' + s + '" aria-hidden="true"></svg>';
    return '<svg class="dwd-icon" width="' + s + '" height="' + s + '" viewBox="0 0 24 24" ' + S + ' aria-hidden="true">' + (ICONS[name] || ICONS.cloud) + "</svg>";
  }

  const fmt = {
    num: (v, d) => (isNum(v) ? nf(d == null ? 0 : d).format(v) : "–"),
    temp: (v, d) => (isNum(v) ? nf(d == null ? 0 : d).format(v) + "°" : "–"),
    pct: (v) => (isNum(v) ? nf(0).format(v) + " %" : "–"),
    mm: (v) => (isNum(v) ? nf(v < 10 ? 1 : 0).format(v) + " mm" : "–"),
    kmh: (v) => (isNum(v) ? nf(0).format(v) + " km/h" : "–"),
    hpa: (v) => (isNum(v) ? nf(0).format(v) + " hPa" : "–"),
    time: (iso) => (iso ? new Date(iso).toLocaleTimeString(LOCALE, { hour: "2-digit", minute: "2-digit", timeZone: TZ }) : "–"),
    localHour,
    isNight,
    hour: (iso) => (iso ? new Date(iso).toLocaleTimeString(LOCALE, { hour: "2-digit", timeZone: TZ }) : "–"),
    day: (iso) => (iso ? new Date(iso).toLocaleDateString(LOCALE, { weekday: "short", day: "2-digit", month: "2-digit", timeZone: TZ }) : "–"),
    dateTime: (iso) => (iso ? new Date(iso).toLocaleString(LOCALE, { weekday: "short", day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit", timeZone: TZ }) : "–"),
    windDir: (deg) => (isNum(deg) ? DIRS[Math.round((((deg % 360) + 360) % 360) / 45) % 8] : "–"),
    beaufort: (kmh) => {
      if (!isNum(kmh)) return null;
      const limits = [1, 6, 12, 20, 29, 39, 50, 62, 75, 89, 103, 118];
      let b = 0;
      while (b < limits.length && kmh >= limits[b]) b++;
      return b;
    },
    ww: wwInfo, // ww(code, iso?) → {label, icon|null}; pass iso for day/night icons
    esc: (s) => String(s == null ? "" : s).replace(/[&<>"']/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[ch])),
  };

  const DWD = {
    theme: "light",
    context: {},
    isHosted: inIframe,
    get data() { return lastData; },
    get result() { return lastResult; },

    /** Register a render callback; fired for every tool result (and replayed if data already arrived). */
    onData(cb) {
      dataCbs.push(cb);
      if (lastData) cb(lastData, lastResult);
    },
    onInput(cb) { inputCbs.push(cb); },
    onContext(cb) { contextCbs.push(cb); },

    /** Call a tool on this MCP server from inside the widget. Resolves with the CallToolResult. */
    async callTool(name, args) {
      return request("tools/call", { name, arguments: args || {} });
    },
    /** Call a tool and re-render all widgets with its structuredContent. */
    async refresh(name, args) {
      const result = await DWD.callTool(name, args);
      if (result && result.isError) throw new Error(((result.content || [])[0] || {}).text || "Tool error");
      emitData(extractData(result), result);
      return result;
    },
    /** Post a user message into the chat (e.g. a follow-up question). */
    sendMessage(text) {
      return request("ui/message", { role: "user", content: [{ type: "text", text }] });
    },
    /** Ask the host to open an external link. */
    openLink(url) {
      return request("ui/open-link", { url }).catch(() => window.open(url, "_blank", "noopener"));
    },
    /** Push data directly (used by the preview harness and standalone dev). */
    render(data) { emitData(data, null); },

    resize: scheduleSize,
    icon,
    fmt,
  };
  window.DWD = DWD;

  // ---- handshake --------------------------------------------------------
  const prefersDark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.dataset.theme = prefersDark ? "dark" : "light";
  DWD.theme = document.documentElement.dataset.theme;
  document.documentElement.dataset.state = "loading";

  if (inIframe) {
    request("ui/initialize", {
      protocolVersion: PROTOCOL_VERSION,
      appInfo: { name: "dwd-widgets", version: "0.1.0" },
      appCapabilities: {},
    })
      .then((res) => {
        applyContext(res && res.hostContext);
        notify("ui/notifications/initialized", {});
        scheduleSize();
      })
      .catch((e) => console.warn("[dwd] ui/initialize failed", e));
  } else {
    // Standalone dev: ?fixture=../fixtures/forecast.json
    const fx = new URLSearchParams(location.search).get("fixture");
    if (fx) fetch(fx).then((r) => r.json()).then((d) => emitData(d, null)).catch(console.error);
  }
})();
