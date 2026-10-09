#!/usr/bin/env node
// Screenshot every DWD widget inside the Claude-like preview host (light + dark), plus a gallery.
//
//   node widgets/preview/capture.mjs --fixtures            # widgets/<name>.html + widgets/fixtures/<name>.json
//   node widgets/preview/capture.mjs --live                # real server: resources/read + tools/call over stdio
//   node widgets/preview/capture.mjs --live --update-fixtures   # also write live structuredContent to fixtures/
//
// Options:
//   --only a,b          scenario ids (see scenarios.mjs), default: all
//   --themes light,dark default: both
//   --width 720         chat column width in CSS px
//   --out DIR           default: docs/screenshots
//   --widgets-dir DIR   fixture mode: read <name>.html + fixtures/ from DIR (shared/ falls back to widgets/shared)
//   --bin PATH          live mode: server binary (default target/release/dwd-mcp-server)
//   --no-gallery        skip docs/screenshots/gallery.png
//   --verbose           print host ⇄ widget JSON-RPC traffic and server stderr
// Exit code 2 if any widget threw / logged console.error / got an isError tool result.
import { mkdirSync, writeFileSync, existsSync, readFileSync } from "node:fs";
import { join, resolve, dirname, relative } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import { parseArgs } from "node:util";
import { SCENARIOS, fixtureName } from "./scenarios.mjs";
import { createFixtureBackend, createLiveBackend, DEFAULT_BIN, REPO_ROOT } from "./backend.mjs";

const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require(process.env.PLAYWRIGHT_PATH || "playwright"));
} catch {
  console.error("Playwright not found. Run `npm i -D playwright && npx playwright install chromium`,\n" +
    "or set PLAYWRIGHT_PATH to an existing playwright package directory.");
  process.exit(1);
}

const here = dirname(fileURLToPath(import.meta.url));
const { values: opt } = parseArgs({
  options: {
    fixtures: { type: "boolean", default: false },
    live: { type: "boolean", default: false },
    "update-fixtures": { type: "boolean", default: false },
    only: { type: "string" },
    themes: { type: "string", default: "light,dark" },
    width: { type: "string", default: "720" },
    out: { type: "string", default: join(REPO_ROOT, "docs/screenshots") },
    "widgets-dir": { type: "string" },
    bin: { type: "string", default: DEFAULT_BIN },
    "no-gallery": { type: "boolean", default: false },
    verbose: { type: "boolean", default: false },
  },
});
if (opt.fixtures === opt.live) {
  console.error("usage: node widgets/preview/capture.mjs (--fixtures | --live [--update-fixtures]) [--only ids] [--out dir]");
  process.exit(1);
}
if (opt["update-fixtures"] && !opt.live) {
  console.error("--update-fixtures only makes sense with --live");
  process.exit(1);
}

const outDir = resolve(opt.out);
const width = Number(opt.width);
const themes = opt.themes.split(",").map((s) => s.trim()).filter(Boolean);
const only = opt.only ? new Set(opt.only.split(",").map((s) => s.trim())) : null;
if (only) {
  const unknown = [...only].filter((id) => !SCENARIOS.some((s) => s.id === id));
  if (unknown.length) {
    console.error(`unknown scenario id(s): ${unknown.join(", ")} — known: ${SCENARIOS.map((s) => s.id).join(", ")}`);
    process.exit(1);
  }
}
const rel = (p) => { const r = relative(process.cwd(), p); return !r || r.startsWith("..") ? p : r; };
// First 3 lines of a message/stack, for readable summaries.
const short = (m) => String(m).split("\n").slice(0, 3).join("\n      ");

const backend = opt.live
  ? await createLiveBackend({ bin: opt.bin, verbose: opt.verbose })
  : createFixtureBackend({ widgetsDir: opt["widgets-dir"] });
const fixturesDir = opt.live ? join(REPO_ROOT, "widgets/fixtures") : backend.fixturesDir;
if (opt.live) console.log(`live: ${backend.serverInfo?.name || "?"} ${backend.serverInfo?.version || ""} · ${backend.tools.size} tools · ${backend.resources.size} ui resources`);

// Optional post-render interactions (scenario.interact), run inside the widget frame like a user would.
const INTERACTIONS = {
  // area-map: move the time slider (keyboard, like a user) to the step with the most precipitation in the area.
  async "wettest-hour"(frame, data) {
    const sums = (data.values || []).map((grid) => grid.flat().reduce((a, v) => a + (typeof v === "number" ? v : 0), 0));
    const idx = sums.reduce((best, v, i) => (v > sums[best] ? i : best), 0);
    if (!sums.length || sums[idx] <= 0) return "no precipitation in any step — slider left at first step";
    await frame.focus(".am-track");
    await frame.press(".am-track", "Home");
    for (let i = 0; i < idx; i++) await frame.press(".am-track", "ArrowRight");
    await frame.evaluate(() => document.activeElement && document.activeElement.blur());
    return `slider → step ${idx} (${data.times[idx]})`;
  },
};

const failures = [];
const warnings = [];
for (const p of backend.problems || []) { console.error("✖", p); failures.push(p); }

mkdirSync(outDir, { recursive: true });
const browser = await chromium.launch();
const hostUrl = pathToFileURL(join(here, "host.html")).href;
let captured = 0;

try {
  for (const sc of await backend.scenarios()) {
    if (only && !only.has(sc.id)) continue;
    if (!sc.available) {
      console.warn(`⚠ skip ${sc.id}: missing ${sc.missing.join(", ")}`);
      warnings.push(`${sc.id}: missing ${sc.missing.join(", ")}`);
      continue;
    }

    let loaded, result;
    try {
      loaded = await backend.load(sc);
      result = await backend.run(sc);
    } catch (e) {
      console.error(`✖ ${sc.id}: ${e.message}`);
      failures.push(`${sc.id}: ${e.message}`);
      continue;
    }
    if (result.isError) {
      const msg = `${sc.id}: ${sc.tool} returned isError: ${JSON.stringify(result.content)}`;
      console.error("✖", msg);
      failures.push(msg);
      continue;
    }
    if (!result.structuredContent) {
      failures.push(`${sc.id}: ${sc.tool} result has no structuredContent`);
    }
    if (opt["update-fixtures"] && result.structuredContent && !sc.illustrative) {
      const f = join(fixturesDir, `${fixtureName(sc)}.json`);
      writeFileSync(f, JSON.stringify(result.structuredContent, null, 2) + "\n");
      console.log(`  fixture → ${rel(f)}`);
    }
    if (opt.live && !sc.illustrative) {
      const text = (result.content || []).filter((c) => c.type === "text").map((c) => c.text).join("\n");
      console.log(`  ${sc.tool} text for the model: ${JSON.stringify(text.slice(0, 200))}`);
    }

    for (const theme of themes) {
      const ctx = await browser.newContext({
        viewport: { width: width + 160, height: 1000 },
        deviceScaleFactor: 2,
        colorScheme: theme === "dark" ? "dark" : "light",
        locale: "de-DE",
        timezoneId: "Europe/Berlin",
      });
      const page = await ctx.newPage();
      const errors = [];
      page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
      page.on("console", (m) => {
        // Errors inside the widget iframe arrive via the host's injected reporter (status.errors); only keep host-page ones here.
        if (m.type() === "error" && !String(m.location().url).startsWith("about:srcdoc")) errors.push(`host console.error: ${m.text()}`);
        else if (opt.verbose) console.log(`  [${m.type()}] ${m.text()}`);
      });
      await page.exposeFunction("__dwdCallTool", (name, args) => backend.callTool(name, args));
      await page.exposeFunction("__dwdLog", (e) => {
        if (opt.verbose) console.log(`  ${e.dir} ${e.method} ${typeof e.detail === "string" ? e.detail : JSON.stringify(e.detail ?? "").slice(0, 160)}`);
      });
      await page.goto(hostUrl);
      await page.evaluate(
        ({ sc, html, prefersBorder, result, theme, width }) => {
          window.DWDHost.mount({
            scenario: sc, html, prefersBorder, result, theme, width, capture: true,
            callTool: (n, a) => window.__dwdCallTool(n, a),
            onLog: (e) => window.__dwdLog(e),
          });
        },
        { sc, html: loaded.html, prefersBorder: loaded.prefersBorder, result, theme, width },
      );
      let timedOut = false;
      try {
        await page.waitForFunction(() => window.DWDHost.current && window.DWDHost.current.status.ready, null, { timeout: 20_000, polling: 100 });
      } catch {
        timedOut = true;
      }
      const status = await page.evaluate(() => {
        const s = window.DWDHost.current.status;
        return { initialized: s.initialized, resultSent: s.resultSent, sizeCount: s.sizeCount, height: s.height, errors: s.errors, warnings: s.warnings };
      });
      if (timedOut) {
        errors.push(
          !status.initialized ? "widget never completed ui/initialize → ui/notifications/initialized"
            : status.sizeCount === 0 ? "widget never sent ui/notifications/size-changed"
            : "widget did not settle within 20 s",
        );
      }
      if (sc.interact && !timedOut) {
        try {
          const frame = await (await page.$(".app iframe")).contentFrame();
          const note = await INTERACTIONS[sc.interact](frame, result.structuredContent);
          if (note) console.log(`  ${sc.id} ${theme}: ${note}`);
          await page.waitForTimeout(300);
        } catch (e) {
          errors.push(`interaction ${sc.interact} failed: ${e.message}`);
        }
      }
      await page.waitForTimeout(250); // let fonts/animations settle
      const file = join(outDir, `${sc.id}-${theme}.png`);
      await page.locator(".chat").screenshot({ path: file });
      await ctx.close();

      for (const e of status.errors) if (!errors.includes(e)) errors.push(e);
      for (const w of new Set(status.warnings)) warnings.push(`${sc.id}/${theme}: ${w}`);
      const tag = errors.length ? "✖" : "✓";
      console.log(`${tag} ${sc.id} ${theme} · ${Math.round(status.height)}px → ${rel(file)}`);
      for (const e of errors) {
        console.error(`    ${short(e)}`);
        failures.push(`${sc.id}/${theme}: ${short(e)}`);
      }
      captured++;
    }
  }

  // ---- gallery: all light shots (also ones from earlier partial runs) ------
  if (!opt["no-gallery"]) {
    const shots = SCENARIOS.filter((s) => !s.illustrative)
      .map((s) => ({ s, f: join(outDir, `${s.id}-light.png`) }))
      .filter((x) => existsSync(x.f));
    if (shots.length) {
      const cards = shots
        .map(({ s, f }) =>
          `<figure><figcaption><code>${s.tool}</code><span>${s.prompt.replace(/</g, "&lt;")}</span></figcaption>` +
          `<img src="data:image/png;base64,${readFileSync(f).toString("base64")}"></figure>`)
        .join("");
      const page = await browser.newPage({ viewport: { width: 1360, height: 900 }, deviceScaleFactor: 1 });
      await page.setContent(`<!doctype html><html><head><meta charset="utf-8"><style>
        body{margin:0;background:#ebe9e1;font-family:ui-sans-serif,system-ui,-apple-system,sans-serif;color:#141413}
        #g{display:grid;grid-template-columns:1fr 1fr;gap:24px;padding:28px;width:1360px;box-sizing:border-box;align-items:start}
        h1{grid-column:1/-1;margin:0;font-size:22px;font-weight:650}h1 small{font-weight:400;color:#73726c;font-size:14px;margin-left:10px}
        figure{margin:0;background:#faf9f5;border-radius:14px;overflow:hidden;box-shadow:0 1px 2px rgba(0,0,0,.06),0 6px 18px rgba(0,0,0,.06)}
        figcaption{display:flex;gap:10px;align-items:baseline;padding:10px 14px;border-bottom:1px solid rgba(0,0,0,.08);font-size:12.5px;color:#73726c}
        figcaption code{font:600 12px ui-monospace,Menlo,monospace;color:#141413}
        img{display:block;width:100%}
      </style></head><body><div id="g"><h1>DWD MCP Apps widgets<small>dwd-rmcp · rendered inline in Claude</small></h1>${cards}</div></body></html>`);
      await page.waitForLoadState("load");
      const file = join(outDir, "gallery.png");
      await page.locator("#g").screenshot({ path: file });
      await page.close();
      console.log(`✓ gallery (${shots.length} shots) → ${rel(file)}`);
    }
  }
} finally {
  await browser.close();
  backend.close();
}

if (!captured) console.warn("⚠ nothing captured (no available widgets)");
if (warnings.length) {
  console.warn(`\n${warnings.length} warning(s):`);
  for (const w of warnings) console.warn("  ⚠ " + w);
}
if (failures.length) {
  console.error(`\n${failures.length} failure(s):`);
  for (const f of failures) console.error("  ✖ " + f);
  process.exit(2);
}
