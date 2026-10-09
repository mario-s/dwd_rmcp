// Mini MCP Apps host: renders a ui:// widget like Claude does (sandboxed srcdoc iframe + postMessage JSON-RPC).
//   window.DWDHost.mount(opts) — used by capture.mjs (Playwright, file://) and by the interactive page (serve.mjs).
// Host side of the protocol:
//   ← ui/initialize (request)              → {protocolVersion, hostInfo, hostCapabilities, hostContext}
//   ← ui/notifications/initialized         → send ui/notifications/tool-input, then ui/notifications/tool-result
//   ← ui/notifications/size-changed        → resize iframe
//   ← tools/call (request)                 → opts.callTool (live: proxied to the real server, fixtures: canned)
//   ← ui/message, ui/open-link (requests)  → logged (+ appended / opened in interactive mode)
//   → ui/notifications/host-context-changed on theme switch, ui/resource-teardown before unmount
(function () {
  "use strict";

  const PROTOCOL_VERSION = "2025-11-21";
  const HOST_INFO = { name: "dwd-preview-host", version: "0.1.0" };

  // Injected as the first thing in <head> so even syntax errors in widget code reach the harness.
  // Uses a non-JSON-RPC message shape, which bridge.js ignores.
  const ERROR_REPORTER =
    "<script>(function(){function r(k,m){try{parent.postMessage({__dwdPreview:k,message:String(m)},'*')}catch(_){}}" +
    "addEventListener('error',function(e){r('error',(e.message||'error')+(e.lineno?' (line '+e.lineno+':'+e.colno+')':''))});" +
    "addEventListener('unhandledrejection',function(e){var x=e.reason;r('error','Unhandled rejection: '+(x&&x.stack||x&&x.message||x))});" +
    "var ce=console.error,cw=console.warn;" +
    "console.error=function(){r('error',[].map.call(arguments,function(a){return a&&a.stack||(typeof a==='object'?JSON.stringify(a):String(a))}).join(' '));return ce.apply(console,arguments)};" +
    "console.warn=function(){r('warn',[].map.call(arguments,String).join(' '));return cw.apply(console,arguments)};" +
    "})();<\/script>";

  function instrument(html) {
    const m = /<head[^>]*>/i.exec(html);
    if (!m) return ERROR_REPORTER + html;
    const at = m.index + m[0].length;
    return html.slice(0, at) + ERROR_REPORTER + html.slice(at);
  }

  // Host style variables in MCP Apps naming (widgets may use them; DWD widgets mostly use their own tokens).
  function styleVariables(theme) {
    const dark = theme === "dark";
    return {
      "--color-background-primary": dark ? "#262624" : "#faf9f5",
      "--color-background-secondary": dark ? "#30302e" : "#f0eee6",
      "--color-text-primary": dark ? "#faf9f5" : "#141413",
      "--color-text-secondary": dark ? "#9c9a92" : "#73726c",
      "--color-border-primary": dark ? "rgba(250,249,245,0.12)" : "rgba(31,30,29,0.12)",
      "--font-sans": "ui-sans-serif, system-ui, -apple-system, sans-serif",
      "--font-mono": "ui-monospace, Menlo, monospace",
      "--border-radius-md": "10px",
    };
  }

  function hostContext(theme, width, toolName) {
    return {
      theme,
      displayMode: "inline",
      availableDisplayModes: ["inline"],
      containerDimensions: { width, maxHeight: 2000 },
      locale: "de-DE",
      timeZone: "Europe/Berlin",
      platform: "web",
      userAgent: "dwd-preview-host/0.1.0",
      deviceCapabilities: { touch: false, hover: true },
      safeAreaInsets: { top: 0, right: 0, bottom: 0, left: 0 },
      toolInfo: toolName ? { tool: { name: toolName } } : undefined,
      styles: { variables: styleVariables(theme) },
    };
  }

  const esc = (s) => String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  const TOOL_ICON =
    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
    '<path d="M14.7 6.3a4 4 0 0 0-5.4 5.2L3.5 17.3a1.8 1.8 0 0 0 2.6 2.6l5.8-5.8a4 4 0 0 0 5.2-5.4l-2.6 2.6-2.4-.4-.4-2.4z"/></svg>';
  const CHEV = '<svg class="chev" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M9 6l6 6-6 6"/></svg>';

  function applyChromeTheme(theme) {
    document.documentElement.dataset.theme = theme;
  }

  let current = null;

  /**
   * opts: { container, scenario: {prompt, assistant, tool, args}, html, result (CallToolResult or Promise),
   *         theme, width, serverName, callTool(name,args)->Promise<CallToolResult>, onLog(entry), interactive, capture }
   */
  function mount(opts) {
    if (current) current.dispose();
    const sc = opts.scenario || {};
    const theme = opts.theme || "light";
    const width = opts.width || 720;
    const onLog = opts.onLog || function () {};
    applyChromeTheme(theme);

    const container = opts.container || document.getElementById("stage");
    container.innerHTML = "";
    const chat = document.createElement("div");
    chat.className = "chat" + (opts.capture ? " capture" : "");
    chat.style.setProperty("--col", width + "px");
    chat.innerHTML =
      '<div class="turn user"><div class="bubble">' + esc(sc.prompt) + "</div></div>" +
      '<div class="turn assistant">' +
      '<div class="toolcall">' + TOOL_ICON + "<span>" + esc(opts.serverName || "dwd-rmcp") + "</span>" + CHEV + "<code>" + esc(sc.tool) + "</code></div>" +
      '<p class="say">' + esc(sc.assistant) + "</p>" +
      '<div class="app"></div>' +
      "</div>";
    container.appendChild(chat);

    const iframe = document.createElement("iframe");
    iframe.setAttribute("sandbox", "allow-scripts");
    iframe.setAttribute("title", sc.tool || "MCP App");
    iframe.setAttribute("scrolling", "no");
    const app = chat.querySelector(".app");
    if (opts.prefersBorder) app.classList.add("bordered"); // resource _meta.ui.prefersBorder
    app.appendChild(iframe);

    const status = {
      theme,
      initialized: false,
      inputSent: false,
      resultSent: false,
      resultSentAt: 0,
      sizeCount: 0,
      lastSizeAt: 0,
      height: 0,
      width: 0,
      errors: [],
      warnings: [],
      messages: [],
      links: [],
      toolCalls: [],
      appInfo: null,
      get ready() {
        return this.resultSent && this.sizeCount > 0 && Date.now() - Math.max(this.lastSizeAt, this.resultSentAt) > 450;
      },
    };

    let nextId = 1;
    const pending = new Map();
    const log = (dir, method, detail, level) => onLog({ t: Date.now(), dir, method, detail, level: level || "info" });

    function post(msg) {
      if (iframe.contentWindow) iframe.contentWindow.postMessage(msg, "*");
    }
    function respond(id, result) { post({ jsonrpc: "2.0", id, result }); }
    function respondError(id, code, message) { post({ jsonrpc: "2.0", id, error: { code, message } }); }
    function notify(method, params) {
      log("→", method, params);
      post({ jsonrpc: "2.0", method, params });
    }
    function request(method, params, timeoutMs) {
      const id = "host-" + nextId++;
      log("→", method, params);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(method + " timed out")); }, timeoutMs || 1000);
        pending.set(id, { resolve, reject, timer });
        post({ jsonrpc: "2.0", id, method, params: params || {} });
      });
    }

    async function deliver() {
      notify("ui/notifications/tool-input", { arguments: sc.args || {} });
      status.inputSent = true;
      let result;
      try {
        result = await opts.result;
      } catch (e) {
        result = { isError: true, content: [{ type: "text", text: String((e && e.message) || e) }] };
      }
      if (disposed) return;
      if (result && result.isError) {
        status.errors.push("tool result isError: " + JSON.stringify(result.content));
      }
      notify("ui/notifications/tool-result", result);
      status.resultSent = true;
      status.resultSentAt = Date.now();
    }

    async function handleRequest(m) {
      const p = m.params || {};
      log("←", m.method, p);
      switch (m.method) {
        case "ui/initialize":
          status.appInfo = p.appInfo || null;
          if (p.protocolVersion && p.protocolVersion !== PROTOCOL_VERSION) {
            status.warnings.push("app protocolVersion " + p.protocolVersion + " ≠ host " + PROTOCOL_VERSION);
          }
          respond(m.id, {
            protocolVersion: PROTOCOL_VERSION,
            hostInfo: HOST_INFO,
            hostCapabilities: {
              openLinks: {},
              serverTools: { listChanged: false },
              serverResources: { listChanged: false },
              logging: {},
            },
            hostContext: hostContext(status.theme, width, sc.tool),
          });
          return;
        case "tools/call": {
          status.toolCalls.push(p);
          let result;
          try {
            result = opts.callTool ? await opts.callTool(p.name, p.arguments || {}) : { isError: true, content: [{ type: "text", text: "tools/call not available" }] };
          } catch (e) {
            result = { isError: true, content: [{ type: "text", text: String((e && e.message) || e) }] };
          }
          if (result && result._meta && result._meta["dwd-preview/warning"]) {
            status.warnings.push(result._meta["dwd-preview/warning"]);
            log("!", "tools/call", result._meta["dwd-preview/warning"], "warn");
          }
          log("→", "tools/call result", result && result.isError ? result.content : { structuredContent: "…" }, result && result.isError ? "warn" : "info");
          respond(m.id, result);
          return;
        }
        case "ui/message":
          status.messages.push(p);
          if (opts.onUserMessage) opts.onUserMessage(p, chat);
          respond(m.id, {});
          return;
        case "ui/open-link":
          status.links.push(p.url);
          if (opts.onOpenLink) opts.onOpenLink(p.url);
          respond(m.id, {});
          return;
        case "ui/request-display-mode":
          respond(m.id, { mode: "inline" });
          return;
        case "ping":
          respond(m.id, {});
          return;
        default:
          log("!", m.method, "unsupported request", "warn");
          respondError(m.id, -32601, "Method not found: " + m.method);
      }
    }

    function handleNotification(m) {
      const p = m.params || {};
      switch (m.method) {
        case "ui/notifications/initialized":
          log("←", m.method, p);
          status.initialized = true;
          deliver();
          return;
        case "ui/notifications/size-changed":
          log("←", m.method, p);
          if (typeof p.height === "number" && p.height > 0) {
            iframe.style.height = Math.ceil(p.height) + "px";
            status.height = p.height;
            status.width = p.width || 0;
            status.sizeCount++;
            status.lastSizeAt = Date.now();
            if (p.height > 560) status.warnings.push("widget height " + Math.round(p.height) + "px > ~520px guideline");
          }
          return;
        default:
          log("←", m.method, p);
      }
    }

    function onMessage(ev) {
      if (ev.source !== iframe.contentWindow) return;
      const m = ev.data;
      if (m && m.__dwdPreview) {
        (m.__dwdPreview === "error" ? status.errors : status.warnings).push(m.message);
        log("!", "widget " + m.__dwdPreview, m.message, m.__dwdPreview === "error" ? "err" : "warn");
        return;
      }
      if (!m || m.jsonrpc !== "2.0") return;
      if (m.method && m.id != null) handleRequest(m);
      else if (m.method) handleNotification(m);
      else if (m.id != null && pending.has(m.id)) {
        const pr = pending.get(m.id);
        pending.delete(m.id);
        clearTimeout(pr.timer);
        log("←", "response " + m.id, m.error || m.result);
        m.error ? pr.reject(m.error) : pr.resolve(m.result);
      }
    }
    window.addEventListener("message", onMessage);

    let disposed = false;
    const ctrl = {
      status,
      chat,
      iframe,
      setTheme(t) {
        status.theme = t;
        applyChromeTheme(t);
        if (status.initialized) notify("ui/notifications/host-context-changed", { theme: t, styles: { variables: styleVariables(t) } });
      },
      async teardown() {
        if (status.initialized && !disposed) {
          try { await request("ui/resource-teardown", {}, 500); } catch (e) { log("!", "ui/resource-teardown", String(e.message || e), "warn"); }
        }
        ctrl.dispose();
      },
      dispose() {
        if (disposed) return;
        disposed = true;
        window.removeEventListener("message", onMessage);
        if (current === ctrl) current = null;
      },
    };
    current = ctrl;
    log("·", "mount", { tool: sc.tool, theme, width });
    iframe.srcdoc = instrument(opts.html || "<p>no html</p>");
    return ctrl;
  }

  window.DWDHost = {
    mount,
    instrument,
    get current() { return current; },
  };

  // ---------------------------------------------------------------------------
  // Interactive mode (served by serve.mjs over http)
  // ---------------------------------------------------------------------------
  if (!/^https?:$/.test(location.protocol) || new URLSearchParams(location.search).has("embed")) return;

  const $ = (id) => document.getElementById(id);
  const qs = new URLSearchParams(location.search);
  const state = {
    id: qs.get("s"),
    theme: qs.get("theme") || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"),
    width: Number(qs.get("w")) || 720,
    showLog: qs.get("log") !== "0",
    scenarios: [],
    mode: "?",
  };
  let errCount = 0;

  function syncUrl() {
    const p = new URLSearchParams({ s: state.id || "", theme: state.theme, w: String(state.width) });
    if (!state.showLog) p.set("log", "0");
    history.replaceState(null, "", "?" + p.toString());
  }

  function logEntry(e) {
    const el = document.createElement("div");
    el.className = "e" + (e.level === "err" ? " err" : e.level === "warn" ? " warn" : "");
    let detail = "";
    if (e.detail !== undefined) {
      detail = typeof e.detail === "string" ? e.detail : JSON.stringify(e.detail);
      if (detail.length > 400) detail = detail.slice(0, 400) + "…";
    }
    el.innerHTML = '<span class="dir">' + esc(e.dir) + "</span> <b>" + esc(e.method) + "</b> " + esc(detail);
    $("log").appendChild(el);
    $("log").scrollTop = $("log").scrollHeight;
    if (e.level === "err") {
      errCount++;
      $("errs").hidden = false;
      $("errs").className = "badge err";
      $("errs").textContent = errCount + " Fehler";
    }
  }

  async function api(path, body) {
    const r = await fetch(path, body ? { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) } : {});
    const j = await r.json();
    if (!r.ok) throw new Error(j.error || r.statusText);
    return j;
  }

  async function show() {
    const sc = state.scenarios.find((s) => s.id === state.id);
    syncUrl();
    if (current) await current.teardown();
    $("log").innerHTML = "";
    errCount = 0;
    $("errs").hidden = true;
    if (!sc) return;
    if (!sc.available) {
      applyChromeTheme(state.theme);
      $("stage").innerHTML = '<div class="notice">Szenario <b>' + esc(sc.id) + "</b> nicht verfügbar — fehlt: " + esc(sc.missing.join(", ")) + "</div>";
      return;
    }
    let loaded;
    try {
      loaded = await api("/api/load?id=" + encodeURIComponent(sc.id));
    } catch (e) {
      $("stage").innerHTML = '<div class="notice">Fehler beim Laden: ' + esc(e.message) + "</div>";
      return;
    }
    mount({
      container: $("stage"),
      scenario: sc,
      html: loaded.html,
      prefersBorder: loaded.prefersBorder,
      result: api("/api/run?id=" + encodeURIComponent(sc.id)),
      theme: state.theme,
      width: state.width,
      interactive: true,
      callTool: (name, args) => api("/api/call", { name, arguments: args }),
      onLog: logEntry,
      onOpenLink: (url) => window.open(url, "_blank", "noopener"),
      onUserMessage: (p, chat) => {
        const text = ((p.content || []).find((c) => c.type === "text") || {}).text || JSON.stringify(p);
        const turn = document.createElement("div");
        turn.className = "turn user";
        turn.innerHTML = '<div class="bubble">' + esc(text) + "</div>";
        chat.appendChild(turn);
      },
    });
  }

  function setTheme(t) {
    state.theme = t;
    $("th-light").setAttribute("aria-pressed", String(t === "light"));
    $("th-dark").setAttribute("aria-pressed", String(t === "dark"));
    if (current) current.setTheme(t);
    else applyChromeTheme(t);
    syncUrl();
  }

  async function boot() {
    $("devbar").hidden = false;
    $("log").hidden = !state.showLog;
    $("toggle-log").setAttribute("aria-pressed", String(state.showLog));
    const info = await api("/api/scenarios");
    state.mode = info.mode;
    state.scenarios = info.scenarios;
    $("mode").textContent = info.mode === "live" ? "live · " + (info.server || "dwd-mcp-server") : "fixtures";
    $("pick").innerHTML = state.scenarios
      .map((s) => '<option value="' + esc(s.id) + '">' + esc(s.id) + (s.available ? "" : " (fehlt)") + " — " + esc(s.tool) + "</option>")
      .join("");
    if (!state.scenarios.some((s) => s.id === state.id)) {
      state.id = (state.scenarios.find((s) => s.available) || state.scenarios[0] || {}).id;
    }
    $("pick").value = state.id;
    $("width").value = String(state.width);
    setTheme(state.theme);

    $("pick").onchange = () => { state.id = $("pick").value; show(); };
    $("th-light").onclick = () => setTheme("light");
    $("th-dark").onclick = () => setTheme("dark");
    $("width").onchange = () => { state.width = Number($("width").value); show(); };
    $("reload").onclick = () => show();
    $("toggle-log").onclick = () => {
      state.showLog = !state.showLog;
      $("log").hidden = !state.showLog;
      $("toggle-log").setAttribute("aria-pressed", String(state.showLog));
      syncUrl();
    };
    addEventListener("keydown", (e) => {
      if (e.target && /SELECT|INPUT|TEXTAREA/.test(e.target.tagName)) return;
      if (e.key === "r") show();
      if (e.key === "t") setTheme(state.theme === "dark" ? "light" : "dark");
    });
    show();
  }

  boot().catch((e) => {
    $("stage").innerHTML = '<div class="notice">Preview-Server nicht erreichbar: ' + esc(e.message) + "</div>";
  });
})();
