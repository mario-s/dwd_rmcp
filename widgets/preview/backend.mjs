// Data/HTML backends for the preview host.
//   fixture mode: widgets/<name>.html with <!--DWD:SHARED--> inlined (like the Rust server), data from fixtures/*.json
//   live mode:    spawns target/release/dwd-mcp-server over stdio and uses its resources + tools for real
import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";
import { SCENARIOS, fixtureName, fixtureForCall } from "./scenarios.mjs";

const here = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = resolve(here, "../..");
export const DEFAULT_BIN = join(REPO_ROOT, "target/release/dwd-mcp-server");
export const APP_MIME = "text/html;profile=mcp-app";
const SHARED_MARKER = "<!--DWD:SHARED-->";
// Stub widgets (created so include_str! compiles before the real widget exists) start with this marker.
export const isPlaceholder = (html) => /^\s*<!--\s*PLACEHOLDER\s*-->/.test(html);

/** Same substitution as the Rust server (literal replace, no `$` patterns). */
export function injectShared(html, sharedDir) {
  const shared =
    `<style>${readFileSync(join(sharedDir, "theme.css"), "utf8")}</style>` +
    `<script>${readFileSync(join(sharedDir, "bridge.js"), "utf8")}</script>`;
  if (!html.includes(SHARED_MARKER)) {
    throw new Error(`missing ${SHARED_MARKER} marker in <head>`);
  }
  return html.split(SHARED_MARKER).join(shared);
}

// ---------------------------------------------------------------------------
// Minimal MCP client over stdio (newline-delimited JSON-RPC 2.0)
// ---------------------------------------------------------------------------
export class McpStdioClient {
  constructor(bin, { args = [], verbose = false } = {}) {
    this.bin = bin;
    this.args = args;
    this.verbose = verbose;
    this.nextId = 1;
    this.pending = new Map();
    this.stderrTail = [];
  }

  start() {
    if (!existsSync(this.bin)) {
      throw new Error(`server binary not found: ${this.bin} (run \`cargo build --release\`)`);
    }
    this.proc = spawn(this.bin, this.args, { stdio: ["pipe", "pipe", "pipe"], env: process.env });
    this.proc.on("exit", (code, sig) => {
      const err = new Error(`MCP server exited (code ${code}, signal ${sig})\n${this.stderrTail.join("\n")}`);
      for (const p of this.pending.values()) p.reject(err);
      this.pending.clear();
      this.exited = true;
    });
    createInterface({ input: this.proc.stderr }).on("line", (line) => {
      this.stderrTail.push(line);
      if (this.stderrTail.length > 40) this.stderrTail.shift();
      if (this.verbose) process.stderr.write(`[server] ${line}\n`);
    });
    createInterface({ input: this.proc.stdout }).on("line", (line) => {
      if (!line.trim()) return;
      let msg;
      try { msg = JSON.parse(line); } catch { if (this.verbose) console.error("[server stdout]", line); return; }
      if (msg.id != null && !msg.method) {
        const p = this.pending.get(msg.id);
        if (!p) return;
        this.pending.delete(msg.id);
        clearTimeout(p.timer);
        if (msg.error) p.reject(Object.assign(new Error(`${p.method}: ${msg.error.message}`), { rpc: msg.error }));
        else p.resolve(msg.result);
      } else if (msg.id != null && msg.method) {
        // Server → client request (ping, roots/list, …): answer minimally.
        const result = msg.method === "roots/list" ? { roots: [] } : {};
        this.write({ jsonrpc: "2.0", id: msg.id, result });
      } else if (this.verbose) {
        console.error("[server notification]", msg.method);
      }
    });
  }

  write(msg) {
    this.proc.stdin.write(JSON.stringify(msg) + "\n");
  }

  request(method, params = {}, timeoutMs = 90_000) {
    if (this.exited) return Promise.reject(new Error("MCP server not running"));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method}: timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer, method });
      this.write({ jsonrpc: "2.0", id, method, params });
    });
  }

  notify(method, params = {}) {
    this.write({ jsonrpc: "2.0", method, params });
  }

  async initialize() {
    this.serverInit = await this.request("initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {
        // MCP Apps capability advertisement (servers may gate UI tools on it).
        extensions: { "io.modelcontextprotocol/ui": { mimeTypes: [APP_MIME] } },
      },
      clientInfo: { name: "dwd-preview-host", version: "0.1.0" },
    });
    this.notify("notifications/initialized");
    return this.serverInit;
  }

  close() {
    if (this.proc && !this.exited) {
      this.proc.stdin.end();
      this.proc.kill();
    }
  }
}

// ---------------------------------------------------------------------------
// Backends — common interface:
//   mode, scenarios(), load(sc) -> {html, resourceUri}, run(sc) -> CallToolResult,
//   callTool(name, args) -> CallToolResult, close()
// ---------------------------------------------------------------------------

function textSummary(data) {
  const loc = data && data.location && data.location.name;
  return `[Fixture] ${data && data.widget ? data.widget : "widget"}${loc ? " · " + loc : ""}`;
}

export function createFixtureBackend({ widgetsDir = join(REPO_ROOT, "widgets"), sharedDir } = {}) {
  widgetsDir = resolve(widgetsDir);
  const fixturesDir = join(widgetsDir, "fixtures");
  sharedDir = sharedDir || (existsSync(join(widgetsDir, "shared/bridge.js")) ? join(widgetsDir, "shared") : join(REPO_ROOT, "widgets/shared"));
  const htmlPath = (w) => join(widgetsDir, `${w}.html`);
  const fixturePath = (f) => join(fixturesDir, `${f}.json`);
  const readFixture = (f) => JSON.parse(readFileSync(fixturePath(f), "utf8"));
  const asResult = (data) => ({ content: [{ type: "text", text: textSummary(data) }], structuredContent: data });

  return {
    mode: "fixtures",
    widgetsDir,
    fixturesDir,
    async scenarios() {
      return SCENARIOS.map((sc) => {
        const missing = [];
        if (!existsSync(htmlPath(sc.widget))) missing.push(`${sc.widget}.html`);
        else if (isPlaceholder(readFileSync(htmlPath(sc.widget), "utf8"))) missing.push(`${sc.widget}.html (placeholder)`);
        if (!existsSync(fixturePath(fixtureName(sc)))) missing.push(`fixtures/${fixtureName(sc)}.json`);
        return { ...sc, available: missing.length === 0, missing };
      });
    },
    async load(sc) {
      // Read fresh every time so edits show up on reload.
      const html = injectShared(readFileSync(htmlPath(sc.widget), "utf8"), sharedDir);
      return { html, resourceUri: `ui://dwd/${sc.widget}.html` };
    },
    async run(sc) {
      return asResult(readFixture(fixtureName(sc)));
    },
    async callTool(name, args) {
      const m = fixtureForCall(name, args);
      if (!m) return { isError: true, content: [{ type: "text", text: `fixture mode: unknown tool ${name}` }] };
      const hit = m.candidates.find((f) => existsSync(fixturePath(f)));
      if (!hit) return { isError: true, content: [{ type: "text", text: `fixture mode: no fixture for ${name}` }] };
      const res = asResult(readFixture(hit));
      if (hit !== m.candidates[0]) {
        res._meta = { "dwd-preview/warning": `no fixture ${m.candidates[0]}.json — returned ${hit}.json instead` };
      }
      return res;
    },
    close() {},
  };
}

export async function createLiveBackend({ bin = DEFAULT_BIN, verbose = false, fixturesDir = join(REPO_ROOT, "widgets/fixtures") } = {}) {
  const client = new McpStdioClient(bin, { verbose });
  client.start();
  try {
    return await connectLiveBackend(client, fixturesDir);
  } catch (e) {
    client.close(); // don't leave the server running when setup fails
    throw e;
  }
}

async function connectLiveBackend(client, fixturesDir) {
  const fixturePath = (f) => join(fixturesDir, `${f}.json`);
  const init = await client.initialize();

  // Tools + their ui resource (tool _meta.ui.resourceUri, legacy alias "ui/resourceUri").
  const tools = new Map();
  let cursor;
  do {
    const page = await client.request("tools/list", cursor ? { cursor } : {});
    for (const t of page.tools || []) tools.set(t.name, t);
    cursor = page.nextCursor;
  } while (cursor);

  // All ui://dwd/* resources, read once (server-injected HTML).
  const resources = new Map();
  const problems = [];
  if (init.capabilities && init.capabilities.resources) {
    let rc;
    do {
      const page = await client.request("resources/list", rc ? { cursor: rc } : {});
      for (const r of page.resources || []) {
        if (!r.uri.startsWith("ui://dwd/")) continue;
        const read = await client.request("resources/read", { uri: r.uri });
        const c = (read.contents || [])[0];
        if (!c) { problems.push(`${r.uri}: empty resources/read`); continue; }
        const text = c.text != null ? c.text : Buffer.from(c.blob || "", "base64").toString("utf8");
        if (c.mimeType !== APP_MIME) problems.push(`${r.uri}: mimeType ${c.mimeType} (expected ${APP_MIME})`);
        // The marker text itself may legitimately appear inside the injected theme.css comment, so check
        // that the shared bridge actually got inlined instead.
        if (!text.includes("window.DWD = DWD")) problems.push(`${r.uri}: shared bridge.js not inlined (${SHARED_MARKER} not replaced?)`);
        resources.set(r.uri, { ...r, text, contentMeta: c._meta });
      }
      rc = page.nextCursor;
    } while (rc);
  } else {
    problems.push("server does not advertise the `resources` capability");
  }

  const resourceUriOf = (t) => (t && t._meta && ((t._meta.ui && t._meta.ui.resourceUri) || t._meta["ui/resourceUri"])) || null;

  return {
    mode: "live",
    client,
    serverInfo: init.serverInfo,
    tools,
    resources,
    problems,
    async scenarios() {
      return SCENARIOS.map((sc) => {
        const t = tools.get(sc.tool);
        const missing = [];
        if (sc.illustrative && !existsSync(fixturePath(fixtureName(sc)))) missing.push(`fixtures/${fixtureName(sc)}.json`);
        if (!t) missing.push(`tool ${sc.tool}`);
        else {
          const uri = resourceUriOf(t);
          if (!uri) missing.push(`${sc.tool} _meta.ui.resourceUri`);
          else if (!resources.has(uri)) missing.push(`resource ${uri}`);
          else if (isPlaceholder(resources.get(uri).text)) missing.push(`${uri} (placeholder)`);
        }
        return { ...sc, available: missing.length === 0, missing };
      });
    },
    async load(sc) {
      const uri = resourceUriOf(tools.get(sc.tool));
      const res = resources.get(uri);
      const ui = (res.contentMeta && res.contentMeta.ui) || (res._meta && res._meta.ui) || {};
      return { html: res.text, resourceUri: uri, prefersBorder: ui.prefersBorder === true };
    },
    async run(sc) {
      if (sc.illustrative) {
        const data = JSON.parse(readFileSync(fixturePath(fixtureName(sc)), "utf8"));
        return { content: [{ type: "text", text: textSummary(data) }], structuredContent: data };
      }
      return client.request("tools/call", { name: sc.tool, arguments: sc.args });
    },
    async callTool(name, args) {
      try {
        return await client.request("tools/call", { name, arguments: args || {} });
      } catch (e) {
        return { isError: true, content: [{ type: "text", text: String(e.message || e) }] };
      }
    },
    close() { client.close(); },
  };
}
