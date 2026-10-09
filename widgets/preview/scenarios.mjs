// Demo scenarios used by capture.mjs (screenshots / docs) and serve.mjs (interactive picker).
// One entry = one chat turn: German user prompt → show_* tool call → widget.
//   id       screenshot base name (docs/screenshots/<id>-light.png)
//   widget   widgets/<widget>.html  /  ui://dwd/<widget>.html
//   fixture  widgets/fixtures/<fixture>.json (defaults to widget)
//   tool     MCP tool name, args = tool arguments (see widgets/CONTRACT.md)

const BERLIN = { latitude: 52.52, longitude: 13.405 };

export const SCENARIOS = [
  {
    id: "current",
    widget: "current",
    tool: "show_current_weather",
    prompt: "Wie ist das Wetter gerade in Berlin?",
    assistant: "Ich habe die aktuellen Wetterdaten für Berlin beim Deutschen Wetterdienst abgerufen.",
    args: { ...BERLIN, location_name: "Berlin" },
  },
  {
    id: "forecast",
    widget: "forecast",
    tool: "show_forecast_chart",
    prompt: "Zeig mir die Vorhersage für Hamburg für die nächsten 24 Stunden",
    assistant: "Ich habe die stündliche ICON-D2-Vorhersage für Hamburg abgerufen.",
    args: { latitude: 53.55, longitude: 9.99, location_name: "Hamburg", hours: 24 },
  },
  {
    id: "wind",
    widget: "wind",
    tool: "show_wind_profile",
    prompt: "Wie windig wird es heute in Kiel?",
    assistant: "Ich habe Wind- und Böenvorhersage für Kiel abgerufen.",
    args: { latitude: 54.32, longitude: 10.14, location_name: "Kiel", hours: 24 },
  },
  {
    id: "storm",
    widget: "storm",
    tool: "show_thunderstorm_risk",
    prompt: "Gibt es heute Gewitter in München?",
    assistant: "Ich habe die Gewitter- und Konvektionsindikatoren für München abgerufen.",
    args: { latitude: 48.14, longitude: 11.58, location_name: "München", hours: 24 },
  },
  {
    id: "compare",
    widget: "compare",
    tool: "show_location_compare",
    prompt: "Vergleiche das Wetter in Berlin, Hamburg, München und Köln für die nächsten 24 Stunden",
    assistant: "Ich habe die Vorhersagen für Berlin, Hamburg, München und Köln abgerufen.",
    args: {
      locations: [
        { name: "Berlin", ...BERLIN },
        { name: "Hamburg", latitude: 53.55, longitude: 9.99 },
        { name: "München", latitude: 48.14, longitude: 11.58 },
        { name: "Köln", latitude: 50.94, longitude: 6.96 },
      ],
      hours: 24,
    },
  },
  {
    id: "area-map",
    widget: "area-map",
    tool: "show_area_map",
    prompt: "Zeig mir eine Temperaturkarte rund um Berlin",
    assistant: "Ich habe das Temperaturfeld im Umkreis von 80 km um Berlin abgerufen.",
    args: { ...BERLIN, radius_km: 80, location_name: "Berlin", parameter: "T_2M" },
  },
  {
    id: "area-map-precip",
    widget: "area-map",
    fixture: "area-map.TOT_PREC",
    tool: "show_area_map",
    prompt: "Wo regnet es in den nächsten Stunden rund um Berlin?",
    assistant: "Ich habe die stündlichen Niederschlagsmengen im Umkreis von 80 km um Berlin abgerufen.",
    args: { ...BERLIN, radius_km: 80, location_name: "Berlin", parameter: "TOT_PREC" },
    // Rain rarely falls exactly "now": scrub the time slider to the wettest hour before the screenshot.
    interact: "wettest-hour",
  },
  {
    id: "model-runs",
    widget: "model-runs",
    tool: "show_model_runs",
    prompt: "Welche DWD-Modelldaten sind verfügbar?",
    assistant: "Ich habe die verfügbaren Modell-Collections und Läufe der DWD-EDR-API abgerufen.",
    args: {},
  },
  // ---- Illustrative scenarios: hand-made payloads (widgets/fixtures/scenarios/*.json), never fetched live.
  // In --live mode the widget HTML still comes from the server; only the data is canned.
  {
    id: "storm-scenario",
    widget: "storm",
    fixture: "scenarios/storm",
    illustrative: true,
    tool: "show_thunderstorm_risk",
    prompt: "Gibt es heute Gewitter in Berlin?",
    assistant: "Ich habe die Gewitter- und Konvektionsindikatoren für Berlin abgerufen.",
    args: { ...BERLIN, location_name: "Berlin", hours: 24 },
  },
];

export const fixtureName = (sc) => sc.fixture || sc.widget;

/** Fixture file name for a tool call made from inside a widget (e.g. area-map parameter switch). */
export function fixtureForCall(name, args) {
  const sc = SCENARIOS.find((s) => s.tool === name);
  if (!sc) return null;
  const candidates = [];
  if (args && args.parameter) candidates.push(`${sc.widget}.${args.parameter}`);
  candidates.push(sc.widget);
  return { widget: sc.widget, candidates };
}
