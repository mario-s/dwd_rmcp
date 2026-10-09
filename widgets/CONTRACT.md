# DWD widget contract

Single source of truth shared by the Rust server (`src/`) and the HTML widgets (`widgets/*.html`).
If you change a shape here, change both sides.

## How widgets reach Claude (MCP Apps)

- Each widget is one self-contained HTML file `widgets/<name>.html`, embedded in the binary with `include_str!`.
- The server exposes it as resource `ui://dwd/<name>.html`, mimeType **`text/html;profile=mcp-app`**,
  and lists it in `resources/list`.
- Before serving, the server replaces the literal marker `<!--DWD:SHARED-->` (must be in `<head>`) with
  `<style>{widgets/shared/theme.css}</style><script>{widgets/shared/bridge.js}</script>`.
- Each `show_*` tool declares `_meta: { "ui": { "resourceUri": "ui://dwd/<name>.html" }, "ui/resourceUri": "ui://dwd/<name>.html" }`
  (second key is the legacy alias some hosts still read).
- Each `show_*` tool returns a `CallToolResult` with
  - `structuredContent`: the JSON payload described below (this is what the widget renders),
  - `content`: one short **text** summary for the model (German or English, 1–4 lines, key numbers), never the raw JSON dump.
- No external network from widgets (no CDN, no fonts, no tiles). All drawing is inline SVG/Canvas. Data arrives via the tool result only;
  a widget may call `DWD.callTool(...)` / `DWD.refresh(...)` to fetch more from *this* server.
- UI language: **German** labels, `de-DE` number/time formatting, timezone `Europe/Berlin` (use `DWD.fmt`).

## Widget file skeleton

```html
<!doctype html>
<html lang="de">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>DWD · Vorhersage</title>
  <!--DWD:SHARED-->
  <style>/* widget-specific CSS, use theme tokens (var(--card), var(--temp) …) */</style>
</head>
<body>
  <div class="dwd-card">
    <div class="dwd-skeleton">Lade DWD-Daten …</div>
    <div class="dwd-content" id="app"></div>
  </div>
  <script>
    DWD.onData((data) => { /* render into #app; must be idempotent (called again on refresh) */ });
  </script>
</body>
</html>
```

`window.DWD` (from `shared/bridge.js`):

| member | purpose |
|---|---|
| `onData(cb)` | `cb(structuredContent, callToolResult)` on every tool result (replayed if already received) |
| `onInput(cb)` | tool arguments as soon as the host sends them |
| `onContext(cb)`, `theme`, `context` | host context; `html[data-theme="dark"]` is set automatically |
| `callTool(name, args)` | call a tool on this server → `CallToolResult` |
| `refresh(name, args)` | `callTool` + re-render every `onData` subscriber with the new `structuredContent` |
| `sendMessage(text)` | post a follow-up user message into the chat |
| `openLink(url)` | open external link through the host |
| `render(data)` | push data manually (preview harness / standalone dev) |
| `resize()` | re-measure height (auto via ResizeObserver; call after async layout) |
| `icon(name, size)` | inline SVG string. names: sun moon partly cloud fog drizzle rain showers sleet snow thunder wind drop gauge thermo warn bolt layers pin clock chat play pause |
| `fmt.*` | `num(v,d) temp(v,d) pct mm kmh hpa time hour day dateTime windDir(deg) beaufort(kmh) ww(code)->{label,icon} esc(str)` |

Standalone dev without a host: open `widgets/<name>.html?fixture=fixtures/<name>.json` through the preview harness
(`widgets/preview/`), which inlines the shared files the same way the server does.

Theme tokens: see `shared/theme.css` (`--card --card-2 --ink --ink-2 --ink-3 --line --accent --temp --dew --rain --cloud --wind --gust --warn-0..4`,
classes `dwd-card dwd-header dwd-badge-icon dwd-title dwd-sub dwd-eyebrow dwd-kpi dwd-chip dwd-seg dwd-btn dwd-footer dwd-logo dwd-error`).
Every widget must look right in light **and** dark, from 360 px to ~760 px wide. Height is content-driven (keep ≤ ~520 px).

## Data payloads (`structuredContent`)

All numbers are already converted to display units. Missing values are `null`. Times are ISO-8601 UTC strings.

### Common envelope (every payload)

```jsonc
{
  "widget": "forecast",                       // widget name, equals the html file name
  "location": { "name": "Berlin", "lat": 52.52, "lon": 13.405 },   // name may be null
  "model": {
    "collection_id": "ICON-D2-RUC@single_level",
    "instance_id": "2026-10-09T08:00:00Z",
    "reference_time": "2026-10-09T08:00:00Z"
  },
  "generated_at": "2026-10-09T08:41:12Z",
  "source": "Deutscher Wetterdienst · ICON-D2-RUC (EDR API)"
}
```

### `Step` (hourly point value, used by several widgets)

```jsonc
{
  "time": "2026-10-09T09:00:00Z",
  "t2m_c": 12.4,            // T_2M K → °C (1 decimal)
  "td2m_c": 8.1,            // TD_2M K → °C
  "precip_mm": 0.3,         // hourly amount = de-accumulated TOT_PREC (never negative)
  "precip_total_mm": 1.2,   // TOT_PREC accumulated since run start
  "cloud_pct": 78,          // CLCT %
  "wind_speed_kmh": 17.6,   // |(U_10M, V_10M)| m/s × 3.6
  "wind_dir_deg": 245,      // meteorological "from" direction: (atan2(-u, -v) in deg + 360) % 360
  "gust_kmh": 38.2,         // VMAX_10M m/s × 3.6
  "pmsl_hpa": 1012.8,       // PMSL Pa / 100
  "ww": 61                  // WW present-weather code (int)
}
```

`Summary` = `{ "t_min_c", "t_max_c", "precip_sum_mm", "gust_max_kmh", "gust_max_time", "cloud_mean_pct" }`.

### 1. `current` — tool `show_current_weather` → `ui://dwd/current.html`
Args: `latitude`, `longitude`, `location_name?`, `collection_id?`, `instance_id?`
```jsonc
{ ...envelope, "widget": "current",
  "now": Step,            // step closest to "now" (>= reference time)
  "next": [Step],         // following 6 hourly steps
  "today": Summary }      // steps on the Europe/Berlin calendar day of `now`
```

### 2. `forecast` — tool `show_forecast_chart` → `ui://dwd/forecast.html`  (meteogram)
Args: `latitude`, `longitude`, `location_name?`, `hours?` (default 24, max 48), `collection_id?`, `instance_id?`
```jsonc
{ ...envelope, "widget": "forecast", "steps": [Step], "summary": Summary }
```

### 3. `wind` — tool `show_wind_profile` → `ui://dwd/wind.html`
Args: as forecast.
```jsonc
{ ...envelope, "widget": "wind", "steps": [Step],
  "summary": { "mean_speed_kmh", "max_speed_kmh", "max_gust_kmh", "max_gust_time", "prevailing_dir_deg" } }
```
`prevailing_dir_deg` = direction of the speed-weighted mean wind vector.

### 4. `storm` — tool `show_thunderstorm_risk` → `ui://dwd/storm.html`  (convection / thunderstorm outlook)
Args: as forecast. Server requests `CAPE_ML, LPI, HAIL_GSP, VMAX_10M, TOT_PREC, WW` (skip any the API rejects).
```jsonc
{ ...envelope, "widget": "storm",
  "steps": [{ "time", "cape_jkg", "lpi", "hail_mm", "gust_kmh", "precip_mm", "ww", "risk" }],
  "summary": { "max_risk", "max_risk_time", "max_cape_jkg", "max_lpi", "max_gust_kmh", "thunder_hours" },
  "levels": [ { "level": 0, "label": "Keine Gefahr" }, { "level": 1, "label": "Gering" },
              { "level": 2, "label": "Erhöht" },      { "level": 3, "label": "Hoch" } ] }
```
`risk` (0–3), first matching rule from the top:
3 if `ww >= 95` or `lpi >= 4` or `gust_kmh >= 90` or `cape_jkg >= 2000`;
2 if `lpi >= 2` or `gust_kmh >= 65` or `cape_jkg >= 1000`;
1 if `lpi > 0` or `cape_jkg >= 300` or `gust_kmh >= 50`;
else 0. `max_risk_time` is `null` when `max_risk` is 0. `thunder_hours` = count of steps with `ww >= 95` or `lpi > 0`.

### 5. `compare` — tool `show_location_compare` → `ui://dwd/compare.html`
Args: `locations: [{ name?, latitude, longitude }]` (2–6), `hours?` (default 24, max 48), `collection_id?`
```jsonc
{ ...envelope, "widget": "compare", "location": null,
  "locations": [ { "name", "lat", "lon", "steps": [Step], "summary": Summary } ] }
```

### 6. `area-map` — tool `show_area_map` → `ui://dwd/area-map.html`  (gridded field over a region, time slider)
Args: `bbox?` ("minx,miny,maxx,maxy") **or** `latitude`+`longitude`+`radius_km?` (default 60, max 250),
`parameter?` one of `T_2M | TOT_PREC | CLCT | VMAX_10M | CAPE_ML` (default `T_2M`), `hours?` (default 12, max 24), `location_name?`.
Server fetches via EDR `cube`, subsamples so the grid is at most 64 × 64, converts units, and (for TOT_PREC) de-accumulates per hour.
```jsonc
{ ...envelope, "widget": "area-map",
  "bbox": [minx, miny, maxx, maxy],
  "parameter": { "name": "T_2M", "label": "Temperatur 2 m", "unit": "°C", "scale": "temp" },   // scale: temp|precip|cloud|wind|cape
  "available_parameters": [ { "name", "label", "unit", "scale" } ],
  "times": [iso],
  "lons": [f64],            // x axis, ascending, length W
  "lats": [f64],            // y axis, ascending, length H
  "values": [[[f64|null]]], // values[t][y][x]
  "stats": { "min": f64, "max": f64 },
  "markers": [ { "name", "lat", "lon" } ] }   // the center/location if given
```
Widget switches parameter with `DWD.refresh("show_area_map", { ...originalArgs, parameter })`.

### 7. `model-runs` — tool `show_model_runs` → `ui://dwd/model-runs.html`  (data-source explorer)
Args: `collection_id?` (default: all collections), `limit?` instances per collection (default 24).
```jsonc
{ ...envelope, "widget": "model-runs", "location": null, "model": null,
  "collections": [ {
      "id", "title", "description",
      "bbox": [minx, miny, maxx, maxy],
      "temporal": { "start", "end", "steps" },   // steps: integer count of forecast time steps
      "parameters": [ { "name", "label", "unit" } ],
      "query_types": ["position", "radius", "area", "cube"],
      "latest_instance": "2026-10-09T08:00:00Z",
      "instances": [iso]          // newest first, max `limit`
  } ] }
```

## Fixtures

`widgets/fixtures/<name>.json` holds one realistic payload per widget (ideally captured live from the server via the
preview harness). Widget authors create a hand-written one first if none exists; it must follow this contract exactly.
