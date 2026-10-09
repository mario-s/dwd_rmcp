# Widget preview host

A small local **MCP Apps host**. It renders the DWD widgets the way Claude does: in a Claude-like chat
frame, inside a `sandbox="allow-scripts"` `srcdoc` iframe, using the real host side of the postMessage
JSON-RPC protocol.

| File | Purpose |
|---|---|
| `host.html` / `host.js` | The host. It answers `ui/initialize` with `hostContext {theme, displayMode: "inline", …}`, sends `tool-input` and then `tool-result` after `ui/notifications/initialized`, resizes the iframe on `size-changed`, serves `tools/call`, and logs `ui/message` / `ui/open-link`. It also injects a tiny error reporter so widget exceptions reach the harness. |
| `serve.mjs` | Interactive dev server on <http://localhost:5178>: scenario picker, light/dark toggle (sent live as `host-context-changed`), width picker, and a JSON-RPC log. Keys: `r` reloads the widget, `t` toggles the theme. |
| `capture.mjs` | Takes Playwright screenshots of every scenario in light and dark at 2× → `docs/screenshots/<id>-<theme>.png`, plus `gallery.png`. |
| `scenarios.mjs` | Demo scenarios: German prompt, tool, args, fixture, and an optional `interact` step (e.g. `wettest-hour` moves the area-map slider to the hour with the most rain). Entries marked `illustrative: true` always use hand-made data from `widgets/fixtures/scenarios/`. |
| `backend.mjs` | Fixture backend (inlines `<!--DWD:SHARED-->` like the server does) and live backend (spawns the server over stdio). |

## Modes

**Fixtures** (default for `serve.mjs`). Reads `widgets/<name>.html` and `widgets/fixtures/<fixture>.json`.
A `tools/call` from a widget returns the matching fixture. With an `args.parameter`, it looks for
`fixtures/<widget>.<PARAM>.json` first, for example `area-map.TOT_PREC.json`.

**Live** (`--live`). Spawns `target/release/dwd-mcp-server` and runs `initialize`, `tools/list` and
`resources/list`, then `resources/read` on every `ui://dwd/*` resource. It resolves each scenario's widget
through the tool's `_meta.ui.resourceUri`, which is how Claude does it, and runs the real `tools/call`.
It flags a wrong MIME type, a `bridge.js` that wasn't inlined, or a missing `resources` capability.
Illustrative scenarios still get their HTML from the server, but their data comes from the fixture.

```sh
node widgets/preview/serve.mjs [--live] [--port 5178] [--bin PATH] [--widgets-dir DIR] [--verbose]

node widgets/preview/capture.mjs --fixtures [--only current,wind] [--themes light,dark] [--width 720]
                                            [--out DIR] [--widgets-dir DIR] [--no-gallery] [--verbose]
node widgets/preview/capture.mjs --live [--update-fixtures] [--bin PATH] [--only …]
```

- `--widgets-dir DIR` points fixture mode at another folder with `<name>.html` and `fixtures/`. `shared/`
  falls back to `widgets/shared`. Use it for throwaway experiments outside the repo.
- `--update-fixtures` (live only) writes each live `structuredContent` to `widgets/fixtures/<fixture>.json`.
  It never touches `widgets/fixtures/scenarios/`, which keeps the original hand-made payload for each widget.
  These are illustrative demo scenarios, e.g. a high-risk thunderstorm day in `scenarios/storm.json`, and are
  not real forecasts. Point a scenario at one with `fixture: "scenarios/<name>"` and `illustrative: true`.
- The gallery includes only live, non-illustrative scenarios.
- Missing or placeholder widgets are skipped with a warning.
- The command exits with code 2 if there are page errors, `console.error`, an incomplete handshake, or an `isError` result.

Playwright is loaded from `node_modules` (`npm i -D playwright && npx playwright install chromium`).
To reuse an existing install elsewhere, set `PLAYWRIGHT_PATH` to that playwright package directory.
