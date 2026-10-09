# Interactive weather widgets (MCP Apps)

`dwd-rmcp` doesn't just return text. Seven `show_*` tools return a small interactive HTML app
that Claude renders **inline in the chat**:

- current conditions
- a meteogram
- a wind profile
- a thunderstorm outlook
- a comparison of several cities
- a gridded map with a time slider
- an explorer for the DWD model runs

All data comes live from the Deutscher Wetterdienst
[EDR API](https://nwp.opendata-api.dwd.de/v1beta1/docs), using the ICON-D2-RUC model (about 2.2 km
resolution, recomputed every hour, forecasts up to about +27 h).

![Gallery of all widgets, rendered in a Claude-like chat frame from live DWD data](screenshots/gallery.png)

- [Use in Claude Desktop](#use-in-claude-desktop)
- [What MCP Apps widgets are](#what-mcp-apps-widgets-are)
- [Architecture](#architecture)
- [How the data is fetched](#how-the-data-is-fetched)
- [Widgets](#widgets): [current](#widget-current) · [forecast](#widget-forecast) · [wind](#widget-wind) · [storm](#widget-storm) · [compare](#widget-compare) · [area-map](#widget-area-map) · [model-runs](#widget-model-runs)
- [The widget contract](#the-widget-contract)
- [Adding a new widget](#adding-a-new-widget)
- [Dev workflow](#dev-workflow)
- [Troubleshooting](#troubleshooting)

All screenshots were captured live from `target/release/dwd-mcp-server` on 9 Oct 2026 (ICON-D2-RUC run
08:00 UTC) with `widgets/preview/capture.mjs --live`. The one exception is the thunderstorm example
marked as an *illustrative scenario*.

## Use in Claude Desktop

1. Build the server with `cargo build --release`.
2. Add it to `claude_desktop_config.json`. On macOS the file is at
   `~/Library/Application Support/Claude/claude_desktop_config.json`, and on Windows at
   `%APPDATA%\Claude\claude_desktop_config.json`.
   ```json
   {
     "mcpServers": {
       "dwd-rmcp": {
         "command": "/path/to/dwd_rmcp/target/release/dwd-mcp-server"
       }
     }
   }
   ```
3. **Fully quit and restart Claude Desktop.** Do the same after every rebuild. The widget HTML is compiled
   into the binary, and Claude Desktop keeps the server process running.
4. Ask for weather in German or English. Claude works out the coordinates itself, and the server's
   instructions tell it to prefer the `show_*` tools when you want to *see* the weather.

| Widget | Deutsch | English |
|---|---|---|
| `show_current_weather` | Wie ist das Wetter gerade in Berlin? | What's the weather like in Berlin right now? |
| `show_forecast_chart` | Zeig mir die Vorhersage für Hamburg für die nächsten 24 Stunden | Show me the forecast for Hamburg for the next 24 hours |
| `show_wind_profile` | Wie windig wird es heute in Kiel? | How windy will it be in Kiel today? |
| `show_thunderstorm_risk` | Gibt es heute Gewitter in München? | Will there be thunderstorms in Munich today? |
| `show_location_compare` | Vergleiche das Wetter in Berlin, Hamburg, München und Köln | Compare the weather in Berlin, Hamburg, Munich and Cologne |
| `show_area_map` | Zeig mir eine Temperaturkarte rund um Berlin / Wo regnet es rund um Berlin? | Show me a temperature map around Berlin / Where will it rain around Berlin? |
| `show_model_runs` | Welche DWD-Modelldaten sind verfügbar? | Which DWD model data is available? |

The widgets themselves are in German, with `de-DE` number formats and times in `Europe/Berlin`.

## What MCP Apps widgets are

[MCP Apps](https://github.com/modelcontextprotocol/ext-apps) is an MCP extension that lets a server ship
UI along with its tools. It works in four steps:

1. The server declares the `io.modelcontextprotocol/ui` extension. It exposes each widget as a
   **resource** with a `ui://` URI and the MIME type `text/html;profile=mcp-app`. `dwd-rmcp` serves
   `ui://dwd/<name>.html` with `_meta.ui = { prefersBorder: false, csp: { connectDomains: [], resourceDomains: [] } }`,
   so the widgets need no network access at all.
2. A tool points at its resource from `_meta`: `"_meta": { "ui": { "resourceUri": "ui://dwd/forecast.html" } }`.
   The tool also sets the legacy alias `"ui/resourceUri"`.
3. When the model calls the tool, the host reads the resource and renders it in a **sandboxed iframe**.
   It then passes the tool arguments and the tool result into the iframe over a `postMessage` JSON-RPC
   channel.
4. The widget can talk back through the same channel:
   - `tools/call` calls more tools on this server. The map uses it to switch parameters.
   - `ui/message` posts a follow-up user message, for example "Bestes Zeitfenster ohne Gewitter?".
   - `ui/open-link` opens a link.
   - `ui/notifications/size-changed` reports the widget's height.

The model still gets a short **text** summary in `content` (1–4 lines with the key numbers). That lets it
answer follow-up questions, and hosts without UI support show that text instead of the widget. The
widget renders `structuredContent`, a typed JSON payload. Here is the text the model gets from
`show_wind_profile`:

```text
Wind Kiel (24 h): Mittel 15 km/h, max. 18 km/h, max. Böe 40 km/h (10.10. 00:00), vorherrschend aus SW (219°).
```

## Architecture

```mermaid
flowchart LR
    subgraph Host["Claude Desktop (MCP Apps host)"]
        Model["Model + chat"]
        Frame["Sandboxed iframe<br/>ui://dwd/&lt;name&gt;.html<br/>bridge.js → window.DWD"]
    end
    Server["dwd-rmcp<br/>dwd-mcp-server (Rust, stdio)"]
    EDR["DWD EDR API<br/>nwp.opendata-api.dwd.de"]

    Model -- "tools/list: show_* with _meta.ui.resourceUri" --> Server
    Model -- "tools/call show_*" --> Server
    Server -- "position / cube / MULTIPOINT / instances (HTTP)" --> EDR
    Server -- "content (text) + structuredContent" --> Model
    Model -- "resources/read ui://dwd/&lt;name&gt;.html" --> Server
    Model -- "srcdoc + hostContext" --> Frame
    Frame <-. "postMessage JSON-RPC" .-> Model
```

The lifecycle of one widget:

```mermaid
sequenceDiagram
    participant U as User
    participant C as Claude (host)
    participant S as dwd-rmcp
    participant D as DWD EDR API
    participant W as Widget iframe

    U->>C: "Zeig mir die Vorhersage für Hamburg"
    C->>S: tools/call show_forecast_chart {latitude, longitude, hours}
    S->>D: newest run with data, then position query (POINT)
    D-->>S: CoverageJSON
    S-->>C: CallToolResult {content: text, structuredContent: payload}
    C->>S: resources/read ui://dwd/forecast.html
    S-->>C: HTML (theme.css + bridge.js inlined)
    C->>W: render in sandbox="allow-scripts"
    W->>C: ui/initialize
    C-->>W: {protocolVersion, hostInfo, hostCapabilities, hostContext{theme, displayMode: inline}}
    W->>C: ui/notifications/initialized
    C->>W: ui/notifications/tool-input {arguments}
    C->>W: ui/notifications/tool-result {content, structuredContent}
    W->>C: ui/notifications/size-changed {height}
    opt user switches the map parameter
        W->>C: tools/call show_area_map {…, parameter: TOT_PREC}
        C->>S: tools/call
        S-->>C: CallToolResult
        C-->>W: result → widget re-renders
    end
```

| Piece | Where | Role |
|---|---|---|
| Widget HTML | `widgets/<name>.html` | One self-contained page per widget (inline SVG or Canvas, no external assets), embedded with `include_str!`. |
| Shared theme + bridge | `widgets/shared/theme.css`, `widgets/shared/bridge.js` | Inlined by the server at the `<!--DWD:SHARED-->` marker. `bridge.js` is the MCP Apps client and exposes `window.DWD`. |
| Widget registry | `src/widgets.rs` | `WIDGETS` list (name, title, tool, HTML), URIs, MIME type, `tool_meta()` and `resource_meta()`. |
| `show_*` tools | `src/show_tools.rs` | Parameter types, fetching, payload assembly and text summaries. Merged into the main router in `src/main.rs`. |
| Data shaping | `src/shaping.rs` | Unit conversion, de-accumulation, summaries, storm risk, mapping the grid onto model cells. |
| EDR client | `src/dwd_client.rs` | HTTP client with caches and selection of the newest run that has data. |
| Contract | [`widgets/CONTRACT.md`](../widgets/CONTRACT.md) | The payload shapes, shared by Rust and the widgets. |
| Fixtures | `widgets/fixtures/*.json` | Live payloads for offline development. `widgets/fixtures/scenarios/*.json` holds hand-made illustrative scenarios. |
| Preview host | `widgets/preview/` | Local MCP Apps host emulator, interactive dev server and screenshot capture. |

## How the data is fetched

- **Model and runs.** The default collection is `ICON-D2-RUC@single_level`. Every tool except
  `show_area_map` and `show_model_runs` also accepts `ICON-D2-RUC-EPS@single_level` (ensemble mean).
  The EDR instance list isn't guaranteed to be in chronological order, so the server sorts the run ids
  itself. A run can also be listed before its data is queryable, so the server probes the newest runs
  (up to 4) with a single-point request and uses the first one that returns data. That choice is cached
  for 2 minutes. If the newest run fails with a server error while a widget is being built, the tool
  retries once with the previous run.
- **Time window.** Point forecasts start at the current hour: the step closest to *now* at or after
  the run's reference time. `hours` counts hours ahead, so you get `hours + 1` hourly steps, and a 24 h
  request returns 25 steps. A run reaches about **+27 h**, so longer requests (max 48) stop at the end of
  the run.
- **Units.** T_2M and TD_2M are converted from K to °C. Wind comes from U_10M/V_10M (m/s → km/h, with
  the meteorological "from" direction), gusts from VMAX_10M (km/h), and pressure from PMSL (Pa → hPa).
  TOT_PREC is accumulated since the run started, so the server de-accumulates it into hourly amounts.
  Missing values are `null`.
- **Ensemble collection.** `ICON-D2-RUC-EPS` has no `WW` (present-weather code). In that case the
  weather symbols and labels are empty, and the thunderstorm rules use only CAPE, LPI and gusts. All
  the other fields work. Parameters are requested in parallel groups: three at a time for
  deterministic runs, and one at a time for the 20-member ensemble.
- **Area maps** (`show_area_map`):
  - The EDR `cube` endpoint accepts at most **10,000 km²**. Areas up to about 9,000 km² use one `cube`
    request. Larger areas are sampled with **MULTIPOINT** position queries at the output grid points,
    sent in chunks of 700 points so the URL stays short. The maximum area is about 500 × 500 km.
  - ICON uses an **unstructured triangular grid**. The CoverageJSON comes back as cell polygons, so the
    server maps each point of a regular lon/lat grid to the triangle that contains it, or to the
    nearest cell centroid within 0.05°.
  - The grid is sized so the whole payload stays at about **48,000 values or fewer** across all time
    steps, for example 59 × 59 × 13 steps for an 80 km radius and 12 hours.

## Widgets

Arguments come from the server's `tools/list`. Optional arguments may be omitted or `null`.

<a id="widget-current"></a>

### Aktuelles Wetter: `show_current_weather`

> *Wie ist das Wetter gerade in Berlin?* · *What's the weather like in Berlin right now?*

The current hour at a glance:

- temperature and the weather symbol and label from `WW`
- wind with direction, gusts with Beaufort, precipitation per hour, cloud cover, pressure and dew point
- *Heute*: today's min/max range, total rain and peak gust
- a strip of the next 6 hours

The **Stündliche Vorhersage** button sends a follow-up message (`ui/message`) asking for the hourly
chart. Resource: `ui://dwd/current.html`.

| Arg | Type | Default | Notes |
|---|---|---|---|
| `latitude` | number | required | WGS84 degrees, e.g. `52.52` |
| `longitude` | number | required | WGS84 degrees, e.g. `13.405` |
| `location_name` | string | — | Name shown in the header. Without it, the widget shows coordinates. |
| `collection_id` | string | `ICON-D2-RUC@single_level` | or `ICON-D2-RUC-EPS@single_level` |
| `instance_id` | string | newest run with data | e.g. `2026-10-09T08:00:00Z` |

<p>
  <img src="screenshots/current-light.png" alt="current widget, light theme" width="49%">
  <img src="screenshots/current-dark.png" alt="current widget, dark theme" width="49%">
</p>

<a id="widget-forecast"></a>

### Vorhersage / Meteogramm: `show_forecast_chart`

> *Zeig mir die Vorhersage für Hamburg für die nächsten 24 Stunden* · *Show me the forecast for Hamburg for the next 24 hours*

An hourly meteogram:

- weather symbols and a cloud-cover band at the top
- temperature and dew-point curves, with min/max marked
- precipitation bars
- wind arrows with speeds
- KPIs for min/max, total precipitation, peak gust and mean cloud cover

The *Temperatur / Niederschlag / Wind* toggle changes which part is emphasised. Point at the chart or
use the arrow keys to see the details for one hour. Resource: `ui://dwd/forecast.html`.

| Arg | Type | Default | Notes |
|---|---|---|---|
| `latitude` | number | required | |
| `longitude` | number | required | |
| `location_name` | string | — | |
| `hours` | integer | `24` | max `48`. The run ends at about +27 h. |
| `collection_id` | string | `ICON-D2-RUC@single_level` | or `ICON-D2-RUC-EPS@single_level` |
| `instance_id` | string | newest run with data | |

<p>
  <img src="screenshots/forecast-light.png" alt="forecast widget, light theme" width="49%">
  <img src="screenshots/forecast-dark.png" alt="forecast widget, dark theme" width="49%">
</p>

<a id="widget-wind"></a>

### Wind: `show_wind_profile`

> *Wie windig wird es heute in Kiel?* · *How windy will it be in Kiel today?*

Mean wind and gusts over time, with dashed lines at 50 and 65 km/h and a direction arrow for each hour.
A compass rose shows how often each direction occurs and the **prevailing direction** (the direction of
the speed-weighted mean wind vector). There is a Beaufort panel for the selected hour, and a time
scrubber with a play button that animates the compass and the chart cursor.
Resource: `ui://dwd/wind.html`.

| Arg | Type | Default | Notes |
|---|---|---|---|
| `latitude` | number | required | |
| `longitude` | number | required | |
| `location_name` | string | — | |
| `hours` | integer | `24` | max `48` |
| `collection_id` | string | `ICON-D2-RUC@single_level` | or `ICON-D2-RUC-EPS@single_level` |
| `instance_id` | string | newest run with data | |

<p>
  <img src="screenshots/wind-light.png" alt="wind widget, light theme" width="49%">
  <img src="screenshots/wind-dark.png" alt="wind widget, dark theme" width="49%">
</p>

<a id="widget-storm"></a>

### Gewitterrisiko: `show_thunderstorm_risk`

> *Gibt es heute Gewitter in München?* · *Will there be thunderstorms in Munich today?*

A convection outlook. The server requests `CAPE_ML`, `LPI` (lightning potential index), `HAIL_GSP`,
`VMAX_10M`, `TOT_PREC` and `WW`, and skips any the collection doesn't offer. It gives each hour a
**risk level from 0 to 3**, using the first rule that matches:

| Level | Label | Rule |
|---|---|---|
| 3 | Hoch | `ww ≥ 95` or `lpi ≥ 4` or `gust ≥ 90 km/h` or `cape ≥ 2000 J/kg` |
| 2 | Erhöht | `lpi ≥ 2` or `gust ≥ 65 km/h` or `cape ≥ 1000 J/kg` |
| 1 | Gering | `lpi > 0` or `cape ≥ 300 J/kg` or `gust ≥ 50 km/h` |
| 0 | Keine Gefahr | otherwise |

The widget shows a verdict banner with the peak time and practical advice, KPIs (max CAPE, max LPI, max
gust and *Gewitterstunden*, the hours with `ww ≥ 95` or `lpi > 0`), an hourly risk strip, and CAPE and
LPI charts with threshold lines. The **Bestes Zeitfenster ohne Gewitter?** button sends a follow-up
message. Resource: `ui://dwd/storm.html`.

| Arg | Type | Default | Notes |
|---|---|---|---|
| `latitude` | number | required | |
| `longitude` | number | required | |
| `location_name` | string | — | |
| `hours` | integer | `24` | max `48` |
| `collection_id` | string | `ICON-D2-RUC@single_level` | The EPS collection has no `WW`, so the risk uses only CAPE, LPI and gusts. |
| `instance_id` | string | newest run with data | |

Live, 9 Oct 2026. A calm autumn day in Munich, so no risk:

<p>
  <img src="screenshots/storm-light.png" alt="storm widget, live data, light theme" width="49%">
  <img src="screenshots/storm-dark.png" alt="storm widget, live data, dark theme" width="49%">
</p>

**Illustrative scenario.** This one uses a hand-made payload
(`widgets/fixtures/scenarios/storm.json`), not a real forecast. It shows how the widget looks on a
summer convection day:

<p>
  <img src="screenshots/storm-scenario-light.png" alt="storm widget, illustrative high-risk scenario, light theme" width="49%">
  <img src="screenshots/storm-scenario-dark.png" alt="storm widget, illustrative high-risk scenario, dark theme" width="49%">
</p>

<a id="widget-compare"></a>

### Ortsvergleich: `show_location_compare`

> *Vergleiche das Wetter in Berlin, Hamburg, München und Köln* · *Compare the weather in Berlin, Hamburg, Munich and Cologne*

Compares 2–6 places using the same run. At the top are *Wärmster / Trockenster / Windigster Ort*
cards. Below them is one row per place: current conditions, min/max, rain and peak gust (the best value
in each category gets a "TOP" badge), and a sparkline on a shared scale with night shading. The
*Temperatur / Niederschlag / Wind* toggle switches the sparklines. Resource: `ui://dwd/compare.html`.

| Arg | Type | Default | Notes |
|---|---|---|---|
| `locations` | array of `{ name?, latitude, longitude }` | required | 2–6 entries |
| `hours` | integer | `24` | max `48` |
| `collection_id` | string | `ICON-D2-RUC@single_level` | |

<p>
  <img src="screenshots/compare-light.png" alt="compare widget, light theme" width="49%">
  <img src="screenshots/compare-dark.png" alt="compare widget, dark theme" width="49%">
</p>

<a id="widget-area-map"></a>

### Gebietskarte: `show_area_map`

> *Zeig mir eine Temperaturkarte rund um Berlin* · *Wo regnet es in den nächsten Stunden rund um Berlin?*

A gridded ICON-D2-RUC field drawn as a smooth raster, with:

- a lat/lon graticule, reference cities and the German border
- a colour legend
- a stats panel: value at the centre, plus area min, max and mean
- a time slider with playback

You can point at the map for values at a location. Switching the parameter (*Temperatur, Niederschlag,
Bewölkung, Böen, CAPE*) calls `show_area_map` again from inside the widget with `tools/call`. See
[How the data is fetched](#how-the-data-is-fetched) for how the grid is built.
Resource: `ui://dwd/area-map.html`.

| Arg | Type | Default | Notes |
|---|---|---|---|
| `latitude`, `longitude` | number | — | Centre point. Use these **or** `bbox`. |
| `radius_km` | number | `60` | Half-width around the centre, clamped to 2–250 |
| `bbox` | string | — | `"minx,miny,maxx,maxy"` (lon,lat,lon,lat). The area can be at most about 500 × 500 km. |
| `parameter` | string | `T_2M` | `T_2M` (°C), `TOT_PREC` (mm per hour), `CLCT` (%), `VMAX_10M` (km/h), `CAPE_ML` (J/kg) |
| `hours` | integer | `12` | 1–24 hours ahead (13 time steps by default) |
| `location_name` | string | — | Label of the centre marker |

The model is always `ICON-D2-RUC@single_level`, using the newest run with data.

<p>
  <img src="screenshots/area-map-light.png" alt="area-map widget, temperature, light theme" width="49%">
  <img src="screenshots/area-map-dark.png" alt="area-map widget, temperature, dark theme" width="49%">
</p>

The `TOT_PREC` capture below is live as well. Rain only arrives in the evening, so the screenshot
harness moved the time slider to the wettest hour, the way a user would:

<p>
  <img src="screenshots/area-map-precip-light.png" alt="area-map widget, hourly precipitation, light theme" width="49%">
  <img src="screenshots/area-map-precip-dark.png" alt="area-map widget, hourly precipitation, dark theme" width="49%">
</p>

<a id="widget-model-runs"></a>

### Modellläufe: `show_model_runs`

> *Welche DWD-Modelldaten sind verfügbar?* · *Which DWD model data is available?*

A data-source explorer for the EDR collections, currently `ICON-D2-RUC` and `ICON-D2-RUC-EPS`. For each
collection it shows:

- a coverage map
- the description
- the latest run and how old it is
- the forecast range (+27 h, 28 steps)
- the supported query types
- a timeline of recent hourly runs and the forecast horizon
- a searchable, filterable parameter table

Clicking a parameter asks Claude to explain it (`ui/message`). Resource: `ui://dwd/model-runs.html`.

| Arg | Type | Default | Notes |
|---|---|---|---|
| `collection_id` | string | all collections | e.g. `ICON-D2-RUC@single_level` |
| `limit` | integer | `24` | Runs listed per collection, newest first |

<p>
  <img src="screenshots/model-runs-light.png" alt="model-runs widget, light theme" width="49%">
  <img src="screenshots/model-runs-dark.png" alt="model-runs widget, dark theme" width="49%">
</p>

## The widget contract

[`widgets/CONTRACT.md`](../widgets/CONTRACT.md) is the single source of truth for both the Rust side and
the HTML side. If you change a shape there, change both sides. It defines:

- **Transport.** One self-contained HTML file per widget, served as `ui://dwd/<name>.html`
  (`text/html;profile=mcp-app`). The `<!--DWD:SHARED-->` marker in `<head>` is replaced with
  `theme.css` and `bridge.js`. Tool `_meta` carries `ui.resourceUri` plus the legacy alias.
- **Results.** `structuredContent` is the payload. `content` is one short text summary, never a dump of
  the raw JSON. Widgets load nothing from the network.
- **Payloads.** A common envelope (`widget`, `location`, `model`, `generated_at`, `source`), the hourly
  `Step` and `Summary` shapes, and one section for each widget. All numbers are already in display units,
  missing values are `null`, and times are ISO-8601 UTC.
- **`window.DWD` API** from `bridge.js`:
  - data and context: `onData`, `onInput`, `onContext`
  - calls back to the host: `callTool`, `refresh`, `sendMessage`, `openLink`
  - helpers: `render`, `resize`, `icon`, and `fmt.*` (German formatters, e.g. `fmt.ww(code, iso)`
    with day/night symbols and `fmt.localHour` / `fmt.isNight`)
- **Theme tokens** (`--card`, `--ink`, `--temp`, `--rain`, `--wind`, `--warn-0..4`, …) and shared classes.
  Widgets must look right in light and dark, from 360 to about 760 px wide.

## Adding a new widget

1. **Define the payload** in [`widgets/CONTRACT.md`](../widgets/CONTRACT.md): the tool name, its
   arguments and the `structuredContent` shape. Reuse the envelope and `Step` where you can.
2. **Write the HTML** in `widgets/<name>.html` from the skeleton in the contract:
   - The `<!--DWD:SHARED-->` marker goes in `<head>`.
   - Render inside `DWD.onData(cb)`, and keep the callback idempotent because refresh calls it again.
   - Use the theme tokens rather than hard-coded colours.
   - No network requests.
3. **Add a fixture**, `widgets/fixtures/<name>.json`, written by hand until the tool exists. Iterate with
   `node widgets/dev/snap.mjs <name>` or `node widgets/preview/serve.mjs`.
4. **Register the resource** by adding an entry to `WIDGETS` in `src/widgets.rs`:
   ```rust
   Widget {
       name: "<name>",
       title: "DWD · …",
       description: "…",
       tool: "show_<…>",
       html: include_str!("../widgets/<name>.html"),
   },
   ```
   `resources/list` and `resources/read` pick it up automatically.
5. **Add the tool** in the `#[tool_router(router = show_router)]` block in `src/show_tools.rs`:
   ```rust
   #[tool(
       name = "show_<…>",
       description = "Show an interactive … (DWD ICON-D2-RUC).",
       meta = widgets::tool_meta("<name>"),
       annotations(read_only_hint = true)
   )]
   pub async fn show_<…>(&self, Parameters(p): Parameters<Show…Params>) -> CallToolResult {
       finish("show_<…>", self.<…>_impl(p).await) // impl returns Ok((payload, text_summary))
   }
   ```
   `finish` turns `(payload, text)` into `structuredContent` plus a text block, and turns errors into
   `isError` results.
6. **Add a demo scenario** to `widgets/preview/scenarios.mjs` (German prompt, tool, args).
7. **Build and capture** with
   `cargo build --release && node widgets/preview/capture.mjs --live --update-fixtures --only <id>`,
   then add a section with the screenshots to this file.

## Dev workflow

| Command | What it does |
|---|---|
| `node widgets/dev/snap.mjs <name> [light\|dark] [width]` | Quickest check: renders one widget with `fixtures/<name>.json` without a host and writes `widgets/dev/out/<name>-<theme>-<width>.png` (gitignored). |
| `node widgets/preview/serve.mjs` | Interactive MCP Apps host at <http://localhost:5178>. Pick a scenario, toggle light/dark (sent live as `host-context-changed`), change the column width (360–760 px) and read the JSON-RPC log. Widget files are re-read on every reload (`r`). |
| `node widgets/preview/serve.mjs --live` | Same, but against `target/release/dwd-mcp-server`: HTML comes from `resources/read`, and every tool call, including calls from inside a widget, goes to DWD. |
| `node widgets/preview/capture.mjs --fixtures` | Screenshots of every scenario in a Claude-like chat frame, light and dark at 2×, written to `docs/screenshots/`, plus `gallery.png`. |
| `node widgets/preview/capture.mjs --live --update-fixtures` | Same with live data. It also writes each live `structuredContent` to `widgets/fixtures/<name>.json`. Illustrative scenarios keep their hand-made data. |

The preview host follows the real protocol:

- It answers `ui/initialize` with a `hostContext` (theme, `displayMode: "inline"`, locale, time zone,
  container size).
- It sends `tool-input` and `tool-result` after `initialized`.
- It resizes the iframe on `size-changed` and proxies `tools/call`.
- It logs `ui/message` and `ui/open-link`.

`capture.mjs` exits with a non-zero code in any of these cases: a widget throws or logs `console.error`,
the handshake never completes, a tool returns `isError`, or a server resource is wrong (MIME type, or
`bridge.js` not inlined). Details are in [`widgets/preview/README.md`](../widgets/preview/README.md).
Both scripts need Node 20+ and Playwright.

## Troubleshooting

**The widget doesn't show up, only text.**
Widgets need a host that supports MCP Apps. `dwd-mcp-server` speaks stdio, so use **Claude Desktop**.
Claude Code in the terminal has no UI surface and shows the tool's text summary, which is expected. If
Claude answers with the plain data tools (`get_point_weather_forecast`, …), ask to *see* or *show* the
weather.

**I changed a widget, but Claude still shows the old one.**
The widgets are compiled into the binary. Run `cargo build --release`, then **fully quit and restart
Claude Desktop**. Starting a new chat isn't enough, because the old server process keeps running.

**The widget is empty or stuck on "Lade DWD-Daten …".**
Reproduce it with `node widgets/preview/serve.mjs --live` and read the log. Common causes:

- an `isError` tool result, such as a point outside the ICON-D2 domain, a parameter the collection
  doesn't offer, or an area that is too large;
- a JavaScript error in `onData`, which shows in red in the log.

**"could not load DWD data – DWD API error (HTTP 5xx)".**
The EDR API is occasionally slow, or a brand-new run isn't queryable yet. The server already falls back
to the previous run. If it still fails, try again a minute later.

**No data for a location.**
ICON-D2 covers Germany and its neighbours. The collection's bounding box is about 4.2°W–20.5°E and
43°N–58.2°N, and the `show_model_runs` widget shows it. The server only checks that coordinates are
valid. Points outside the model domain come back from the DWD API as errors or empty values, and area
maps that reach past the edge show empty cells.

**The widget is cut off or has a scrollbar.**
The host sizes the iframe from `ui/notifications/size-changed`, which `bridge.js` sends automatically.
If a layout change happens asynchronously, call `DWD.resize()` afterwards.

**The dark theme looks wrong.**
`bridge.js` sets `html[data-theme]` from `hostContext.theme` and from `host-context-changed`. Use theme
tokens, and toggle the theme live in `serve.mjs` to check.
