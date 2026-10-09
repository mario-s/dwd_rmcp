// Quick visual check for a widget without an MCP host.
// Usage: node widgets/dev/snap.mjs <name> [light|dark] [width] [outfile]
// Inlines shared/theme.css + shared/bridge.js exactly like the Rust server, feeds fixtures/<name>.json
// via DWD.render(), and writes a PNG (default: widgets/dev/out/<name>-<theme>-<width>.png).
import { readFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

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
const root = resolve(here, "..");
const [name, theme = "light", width = "640", outArg] = process.argv.slice(2);
if (!name) {
  console.error("usage: node widgets/dev/snap.mjs <name> [light|dark] [width] [outfile]");
  process.exit(1);
}

const shared =
  `<style>${readFileSync(join(root, "shared/theme.css"), "utf8")}</style>` +
  `<script>${readFileSync(join(root, "shared/bridge.js"), "utf8")}</script>`;
const html = readFileSync(join(root, `${name}.html`), "utf8").replace("<!--DWD:SHARED-->", shared);
const fixture = JSON.parse(readFileSync(join(root, `fixtures/${name}.json`), "utf8"));

const out = outArg || join(here, "out", `${name}-${theme}-${width}.png`);
mkdirSync(dirname(out), { recursive: true });

const browser = await chromium.launch();
const page = await browser.newPage({
  viewport: { width: Number(width), height: 900 },
  deviceScaleFactor: 2,
  colorScheme: theme === "dark" ? "dark" : "light",
});
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
await page.setContent(html, { waitUntil: "load" });
await page.evaluate(
  ({ data, theme }) => {
    document.documentElement.dataset.theme = theme;
    document.body.style.background = theme === "dark" ? "#0b0f17" : "#eef1f7";
    document.body.style.padding = "16px";
    window.DWD.render(data);
  },
  { data: fixture, theme },
);
await page.waitForTimeout(400);
const body = await page.$("body");
await body.screenshot({ path: out });
await browser.close();
console.log(out);
if (errors.length) {
  console.error("page errors:\n" + errors.join("\n"));
  process.exit(2);
}
