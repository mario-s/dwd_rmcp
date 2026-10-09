//! Registry of the MCP Apps widgets (`ui://dwd/<name>.html`).
//!
//! The HTML files live in `widgets/` and are embedded at compile time. Before a
//! widget is served, the `<!--DWD:SHARED-->` marker is replaced with the shared
//! theme stylesheet and the host bridge script (see `widgets/CONTRACT.md`).

use rmcp::model::MetaObject;
use serde_json::{Value, json};

/// MIME type for MCP Apps HTML resources.
pub const MIME_TYPE: &str = "text/html;profile=mcp-app";

/// Marker in each widget `<head>` that is replaced by the shared assets.
pub const SHARED_MARKER: &str = "<!--DWD:SHARED-->";

const THEME_CSS: &str = include_str!("../widgets/shared/theme.css");
const BRIDGE_JS: &str = include_str!("../widgets/shared/bridge.js");

pub struct Widget {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    html: &'static str,
}

impl Widget {
    pub fn uri(&self) -> String {
        format!("ui://dwd/{}.html", self.name)
    }

    /// The widget HTML with the shared theme + bridge inlined.
    pub fn render(&self) -> String {
        let shared = format!("<style>{THEME_CSS}</style><script>{BRIDGE_JS}</script>");
        self.html.replacen(SHARED_MARKER, &shared, 1)
    }
}

pub const WIDGETS: &[Widget] = &[
    Widget {
        name: "current",
        title: "DWD · Aktuelles Wetter",
        description: "Aktuelle Bedingungen und die nächsten Stunden für einen Ort (ICON-D2-RUC).",
        html: include_str!("../widgets/current.html"),
    },
    Widget {
        name: "forecast",
        title: "DWD · Vorhersage",
        description: "Meteogramm: Temperatur, Taupunkt, Niederschlag, Wolken, Wind und Druck stündlich.",
        html: include_str!("../widgets/forecast.html"),
    },
    Widget {
        name: "wind",
        title: "DWD · Wind",
        description: "Windgeschwindigkeit, Böen und Windrichtung im Verlauf mit vorherrschender Richtung.",
        html: include_str!("../widgets/wind.html"),
    },
    Widget {
        name: "storm",
        title: "DWD · Gewitterrisiko",
        description: "Konvektions-/Gewitterausblick aus CAPE, Blitzpotenzial (LPI), Hagel, Böen und Wettercode.",
        html: include_str!("../widgets/storm.html"),
    },
    Widget {
        name: "compare",
        title: "DWD · Ortsvergleich",
        description: "Vergleich der Vorhersage für 2–6 Orte nebeneinander.",
        html: include_str!("../widgets/compare.html"),
    },
    Widget {
        name: "area-map",
        title: "DWD · Gebietskarte",
        description: "Gitterfeld (Temperatur, Niederschlag, Wolken, Böen, CAPE) über einer Region mit Zeitschieber.",
        html: include_str!("../widgets/area-map.html"),
    },
    Widget {
        name: "model-runs",
        title: "DWD · Modellläufe",
        description: "Datenquellen-Explorer: Kollektionen, Parameter und verfügbare Modellläufe der DWD EDR API.",
        html: include_str!("../widgets/model-runs.html"),
    },
];

pub fn find_by_uri(uri: &str) -> Option<&'static Widget> {
    WIDGETS.iter().find(|w| w.uri() == uri)
}

fn to_meta(value: Value) -> MetaObject {
    match value {
        Value::Object(map) => MetaObject(map),
        _ => MetaObject::new(),
    }
}

/// `_meta` for a `show_*` tool: new-style `ui.resourceUri` plus the legacy alias.
pub fn tool_meta(name: &str) -> MetaObject {
    let uri = format!("ui://dwd/{name}.html");
    to_meta(json!({
        "ui": { "resourceUri": uri },
        "ui/resourceUri": uri,
    }))
}

/// `_meta` for a widget resource (list + read).
pub fn resource_meta() -> MetaObject {
    to_meta(json!({
        "ui": {
            "prefersBorder": false,
            "csp": { "connectDomains": [], "resourceDomains": [] }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_widget_has_marker_and_renders() {
        for w in WIDGETS {
            assert!(w.html.contains(SHARED_MARKER), "{} lacks marker", w.name);
            assert!(
                !w.html.replacen(SHARED_MARKER, "", 1).contains(SHARED_MARKER),
                "{} has more than one marker",
                w.name
            );
            let html = w.render();
            assert!(html.contains(THEME_CSS) && html.contains(BRIDGE_JS));
            // the marker only survives inside the injected shared files' comments
            assert_eq!(
                html.matches(SHARED_MARKER).count(),
                THEME_CSS.matches(SHARED_MARKER).count() + BRIDGE_JS.matches(SHARED_MARKER).count()
            );
            assert!(find_by_uri(&w.uri()).is_some());
        }
    }

    #[test]
    fn tool_meta_has_both_keys() {
        let m = tool_meta("forecast");
        assert_eq!(m["ui"]["resourceUri"], "ui://dwd/forecast.html");
        assert_eq!(m["ui/resourceUri"], "ui://dwd/forecast.html");
    }
}
