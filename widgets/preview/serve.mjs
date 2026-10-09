#!/usr/bin/env node
// Interactive preview host for DWD MCP Apps widgets.
//   node widgets/preview/serve.mjs                 → http://localhost:5178 (fixtures, files re-read on every reload)
//   node widgets/preview/serve.mjs --live          → real server over stdio (target/release/dwd-mcp-server)
// Options: --port N, --bin PATH, --widgets-dir DIR (fixture mode), --verbose
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { createFixtureBackend, createLiveBackend, DEFAULT_BIN } from "./backend.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const { values: opt } = parseArgs({
  options: {
    live: { type: "boolean", default: false },
    port: { type: "string", default: "5178" },
    bin: { type: "string", default: DEFAULT_BIN },
    "widgets-dir": { type: "string" },
    verbose: { type: "boolean", default: false },
  },
});

const backend = opt.live
  ? await createLiveBackend({ bin: opt.bin, verbose: opt.verbose })
  : createFixtureBackend({ widgetsDir: opt["widgets-dir"] });
if (backend.problems) for (const p of backend.problems) console.warn("⚠", p);

const scenarioById = async (id) => (await backend.scenarios()).find((s) => s.id === id);

function send(res, status, body, type = "application/json; charset=utf-8") {
  res.writeHead(status, { "content-type": type, "cache-control": "no-store" });
  res.end(typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body));
}

async function readBody(req) {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  return chunks.length ? JSON.parse(Buffer.concat(chunks).toString("utf8")) : {};
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://localhost");
  try {
    if (url.pathname === "/" || url.pathname === "/host.html") {
      return send(res, 200, readFileSync(join(here, "host.html")), "text/html; charset=utf-8");
    }
    if (url.pathname === "/host.js") {
      return send(res, 200, readFileSync(join(here, "host.js")), "text/javascript; charset=utf-8");
    }
    if (url.pathname === "/api/scenarios") {
      return send(res, 200, {
        mode: backend.mode,
        server: backend.serverInfo ? `${backend.serverInfo.name} ${backend.serverInfo.version || ""}`.trim() : null,
        scenarios: await backend.scenarios(),
      });
    }
    if (url.pathname === "/api/load" || url.pathname === "/api/run") {
      const sc = await scenarioById(url.searchParams.get("id"));
      if (!sc) return send(res, 404, { error: "unknown scenario" });
      if (!sc.available) return send(res, 404, { error: "missing: " + sc.missing.join(", ") });
      return send(res, 200, url.pathname === "/api/load" ? await backend.load(sc) : await backend.run(sc));
    }
    if (url.pathname === "/api/call" && req.method === "POST") {
      const { name, arguments: args } = await readBody(req);
      if (opt.verbose) console.log("tools/call", name, JSON.stringify(args));
      return send(res, 200, await backend.callTool(name, args));
    }
    send(res, 404, { error: "not found" });
  } catch (e) {
    console.error(e);
    send(res, 500, { error: String(e.message || e) });
  }
});

server.listen(Number(opt.port), () => {
  console.log(`DWD widget preview (${backend.mode}) → http://localhost:${opt.port}`);
  console.log("keys: r = reload widget, t = toggle theme");
});
const shutdown = () => { backend.close(); server.close(); process.exit(0); };
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
