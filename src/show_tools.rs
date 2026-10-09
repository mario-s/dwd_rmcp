//! `show_*` tools: each returns a `CallToolResult` whose `structuredContent`
//! is rendered by the matching `ui://dwd/<name>.html` widget (MCP Apps).
//! Payload shapes: `widgets/CONTRACT.md`.

use std::future::Future;

use chrono::{DateTime, Datelike, Duration, TimeZone, Utc, Weekday};
use futures::future::join_all;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    schemars::JsonSchema,
    tool, tool_router,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::DwdMcpServer;
use crate::dwd_client::{DEFAULT_COLLECTION, HttpError, is_client_error};
use crate::shaping::{self, BBox, Coverage, Series, Step};
use crate::widgets;

// -----------------------------------------------------------------------------
// Parameters
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowCurrentParams {
    #[schemars(description = "Latitude in degrees (WGS84), e.g. 52.52 for Berlin")]
    pub latitude: f64,
    #[schemars(description = "Longitude in degrees (WGS84), e.g. 13.405 for Berlin")]
    pub longitude: f64,
    #[schemars(description = "Display name of the place, e.g. 'Berlin'")]
    pub location_name: Option<String>,
    #[schemars(description = "Model collection, default ICON-D2-RUC@single_level (or ICON-D2-RUC-EPS@single_level for the ensemble mean)")]
    pub collection_id: Option<String>,
    #[schemars(description = "Model run (instance) id, e.g. 2026-10-09T08:00:00Z. Default: newest run with data.")]
    pub instance_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowSeriesParams {
    #[schemars(description = "Latitude in degrees (WGS84), e.g. 52.52 for Berlin")]
    pub latitude: f64,
    #[schemars(description = "Longitude in degrees (WGS84), e.g. 13.405 for Berlin")]
    pub longitude: f64,
    #[schemars(description = "Display name of the place, e.g. 'Berlin'")]
    pub location_name: Option<String>,
    #[schemars(description = "Hours ahead from now (default 24, max 48; the model run reaches about +27 h)")]
    pub hours: Option<u32>,
    #[schemars(description = "Model collection, default ICON-D2-RUC@single_level (or ICON-D2-RUC-EPS@single_level for the ensemble mean)")]
    pub collection_id: Option<String>,
    #[schemars(description = "Model run (instance) id, e.g. 2026-10-09T08:00:00Z. Default: newest run with data.")]
    pub instance_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompareLocation {
    #[schemars(description = "Display name, e.g. 'Hamburg'")]
    pub name: Option<String>,
    #[schemars(description = "Latitude in degrees (WGS84)")]
    pub latitude: f64,
    #[schemars(description = "Longitude in degrees (WGS84)")]
    pub longitude: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowCompareParams {
    #[schemars(description = "2 to 6 places to compare, each { name?, latitude, longitude }")]
    pub locations: Vec<CompareLocation>,
    #[schemars(description = "Hours ahead from now (default 24, max 48)")]
    pub hours: Option<u32>,
    #[schemars(description = "Model collection, default ICON-D2-RUC@single_level")]
    pub collection_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowAreaMapParams {
    #[schemars(description = "Bounding box 'minx,miny,maxx,maxy' in degrees (lon,lat,lon,lat). Alternative to latitude+longitude+radius_km.")]
    pub bbox: Option<String>,
    #[schemars(description = "Center latitude in degrees (used when bbox is not given)")]
    pub latitude: Option<f64>,
    #[schemars(description = "Center longitude in degrees (used when bbox is not given)")]
    pub longitude: Option<f64>,
    #[schemars(description = "Half-width of the map around the center in km (default 60, max 250)")]
    pub radius_km: Option<f64>,
    #[schemars(description = "Field to map: T_2M (temperature, default), TOT_PREC (hourly precipitation), CLCT (cloud cover), VMAX_10M (gusts), CAPE_ML (convective energy)")]
    pub parameter: Option<String>,
    #[schemars(description = "Hours ahead from now for the time slider (default 12, max 24)")]
    pub hours: Option<u32>,
    #[schemars(description = "Display name of the center place, e.g. 'Berlin'")]
    pub location_name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowModelRunsParams {
    #[schemars(description = "Only this collection, e.g. ICON-D2-RUC@single_level (default: all collections)")]
    pub collection_id: Option<String>,
    #[schemars(description = "Max number of model runs (instances) listed per collection, newest first (default 24)")]
    pub limit: Option<u32>,
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

type ToolOutput = anyhow::Result<(Value, String)>;

fn finish(widget: &str, res: ToolOutput) -> CallToolResult {
    match res {
        Ok((payload, text)) => {
            let mut r = CallToolResult::success(vec![ContentBlock::text(text)]);
            r.structured_content = Some(payload);
            r
        }
        Err(err) => {
            let detail = match err.downcast_ref::<HttpError>() {
                Some(h) => format!("DWD API error (HTTP {}): {}", h.status, h.detail()),
                None => format!("{err:#}"),
            };
            CallToolResult::error(vec![ContentBlock::text(format!(
                "{widget}: could not load DWD data – {detail}"
            ))])
        }
    }
}

fn is_gateway_timeout(err: &anyhow::Error) -> bool {
    err.downcast_ref::<HttpError>().is_some_and(|h| h.status == 504)
}

fn short_collection(id: &str) -> &str {
    id.split('@').next().unwrap_or(id)
}

fn envelope(widget: &str, location: Value, model: Value, collection_id: Option<&str>) -> Map<String, Value> {
    let source = match collection_id {
        Some(c) => format!("Deutscher Wetterdienst · {} (EDR API)", short_collection(c)),
        None => "Deutscher Wetterdienst · Open Data (EDR API)".to_string(),
    };
    let mut m = Map::new();
    m.insert("widget".into(), json!(widget));
    m.insert("location".into(), location);
    m.insert("model".into(), model);
    m.insert("generated_at".into(), json!(shaping::iso(Utc::now())));
    m.insert("source".into(), json!(source));
    m
}

fn location_json(name: Option<&str>, lat: f64, lon: f64) -> Value {
    json!({ "name": name, "lat": lat, "lon": lon })
}

fn model_json(collection_id: &str, instance_id: &str, reference_time: Option<&str>) -> Value {
    json!({
        "collection_id": collection_id,
        "instance_id": instance_id,
        "reference_time": reference_time.unwrap_or(instance_id),
    })
}

fn validate_point(lat: f64, lon: f64) -> anyhow::Result<()> {
    if !(lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
        anyhow::bail!("invalid coordinates lat={lat}, lon={lon}");
    }
    Ok(())
}

fn place(name: Option<&str>, lat: f64, lon: f64) -> String {
    match name {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => format!("{lat:.3}, {lon:.3}"),
    }
}

/// Europe/Berlin wall-clock offset (CET/CEST) for a UTC instant.
fn berlin_offset_hours(t: DateTime<Utc>) -> i64 {
    fn last_sunday(year: i32, month: u32) -> DateTime<Utc> {
        let mut d = Utc.with_ymd_and_hms(year, month, 31, 1, 0, 0).unwrap();
        while d.weekday() != Weekday::Sun {
            d -= Duration::days(1);
        }
        d
    }
    let y = t.year();
    if t >= last_sunday(y, 3) && t < last_sunday(y, 10) { 2 } else { 1 }
}

fn local_time(iso: &str) -> String {
    match shaping::parse_time(iso) {
        Some(t) => {
            let l = t + Duration::hours(berlin_offset_hours(t));
            l.format("%d.%m. %H:%M").to_string()
        }
        None => iso.to_string(),
    }
}

fn num(v: Option<f64>, decimals: usize) -> String {
    match v {
        Some(x) => format!("{x:.decimals$}"),
        None => "–".into(),
    }
}

fn step_window<T: Clone>(items: &[T], start: usize, hours: u32) -> Vec<T> {
    items.iter().skip(start).take(hours as usize + 1).cloned().collect()
}

impl DwdMcpServer {
    /// Run `f` against the requested instance, or against the newest run with
    /// data (falling back to the previous run if the newest one fails).
    async fn with_instance<T, F, Fut>(&self, collection: &str, instance: Option<&str>, f: F) -> anyhow::Result<(String, T)>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        if let Some(id) = instance {
            return f(id.to_string()).await.map(|t| (id.to_string(), t));
        }
        let latest = self.client.get_latest_instance_id(collection).await?;
        match f(latest.clone()).await {
            Ok(t) => Ok((latest, t)),
            Err(e) if !is_client_error(&e) && !is_gateway_timeout(&e) => {
                let older = self
                    .client
                    .instance_candidates(collection)
                    .await
                    .ok()
                    .and_then(|c| c.into_iter().find(|i| i.id < latest));
                match older {
                    Some(prev) => f(prev.id.clone()).await.map(|t| (prev.id, t)).map_err(|_| e),
                    None => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn available_params(&self, collection: &str, wanted: &[&str]) -> anyhow::Result<Vec<String>> {
        let names = self.client.collection_param_names(collection).await?;
        let params: Vec<String> = wanted
            .iter()
            .filter(|w| names.iter().any(|n| n == *w))
            .map(|w| w.to_string())
            .collect();
        if params.is_empty() {
            anyhow::bail!("collection {collection} offers none of the parameters {}", wanted.join(", "));
        }
        Ok(params)
    }

    /// Full-run point time series for `wanted` parameters (filtered to those the collection offers).
    /// Parameters are fetched in parallel groups (the API cost grows with the number of
    /// parameters, especially for the 20-member ensemble) and merged.
    async fn point_series(&self, collection: &str, instance: Option<&str>, lat: f64, lon: f64, wanted: &[&str]) -> anyhow::Result<(String, Series)> {
        validate_point(lat, lon)?;
        let params = self.available_params(collection, wanted).await?;
        let group = if collection.contains("EPS") { 1 } else { 3 };
        let groups: Vec<String> = params.chunks(group).map(|g| g.join(",")).collect();
        let coords = format!("POINT({lon} {lat})");
        self.with_instance(collection, instance, |inst| {
            let groups = groups.clone();
            let coords = coords.clone();
            async move {
                let parts = join_all(groups.iter().map(|g| {
                    self.client
                        .get_position_raw(collection, &coords, Some(&inst), Some(g), None, None, "CoverageJSON")
                }))
                .await;
                let mut series = Series::default();
                for part in parts {
                    let cov = shaping::parse_coverage(&part?).map_err(|_| {
                        anyhow::anyhow!("no model data at {lat}, {lon} (outside the ICON-D2 domain?)")
                    })?;
                    let s = cov.point_series();
                    if series.times.is_empty() {
                        series.times = s.times;
                        series.reference_time = s.reference_time;
                    } else if series.times != s.times {
                        anyhow::bail!("inconsistent time axes in DWD response");
                    }
                    series.values.extend(s.values);
                }
                if series.values.values().all(|v| v.iter().all(Option::is_none)) {
                    anyhow::bail!("no model data at {lat}, {lon} (outside the ICON-D2 domain?)");
                }
                Ok(series)
            }
        })
        .await
    }

    // --- tool bodies ---------------------------------------------------------

    async fn current_impl(&self, p: ShowCurrentParams) -> ToolOutput {
        let collection = p.collection_id.as_deref().unwrap_or(DEFAULT_COLLECTION);
        let (inst, series) = self
            .point_series(collection, p.instance_id.as_deref(), p.latitude, p.longitude, shaping::STEP_PARAMS)
            .await?;
        let steps = shaping::build_steps(&series);
        let i = shaping::now_index(&series.times, series.reference_time.as_deref(), Utc::now());
        let now = steps.get(i).cloned().ok_or_else(|| anyhow::anyhow!("empty forecast"))?;
        let next: Vec<Step> = steps.iter().skip(i + 1).take(6).cloned().collect();
        // "Heute" = the local (Europe/Berlin) calendar day of `now`, not the whole +27 h run.
        let local_day = |iso: &str| {
            shaping::parse_time(iso).map(|t| (t + Duration::hours(berlin_offset_hours(t))).date_naive())
        };
        let now_day = local_day(&now.time);
        let today_steps: Vec<Step> =
            steps.iter().filter(|s| local_day(&s.time) == now_day).cloned().collect();
        let today = shaping::summarize(&today_steps);

        let mut m = envelope(
            "current",
            location_json(p.location_name.as_deref(), p.latitude, p.longitude),
            model_json(collection, &inst, series.reference_time.as_deref()),
            Some(collection),
        );
        m.insert("now".into(), json!(now));
        m.insert("next".into(), json!(next));
        m.insert("today".into(), json!(today));

        let wind = match (now.wind_speed_kmh, now.wind_dir_deg) {
            (Some(s), Some(d)) => format!("Wind {s:.0} km/h aus {}", shaping::compass_de(d as f64)),
            (Some(s), None) => format!("Wind {s:.0} km/h"),
            _ => "Wind –".into(),
        };
        let text = format!(
            "{} – {} (Ortszeit): {} °C, {}, Böen {} km/h, Bewölkung {} %, Niederschlag {} mm/h.\n\
             Heute: {}…{} °C, Σ {} mm, max. Böe {} km/h. Modelllauf {} {}. Das Widget zeigt die Details.",
            place(p.location_name.as_deref(), p.latitude, p.longitude),
            local_time(&now.time),
            num(now.t2m_c, 1),
            wind,
            num(now.gust_kmh, 0),
            now.cloud_pct.map(|c| c.to_string()).unwrap_or("–".into()),
            num(now.precip_mm, 1),
            num(today.t_min_c, 1),
            num(today.t_max_c, 1),
            num(today.precip_sum_mm, 1),
            num(today.gust_max_kmh, 0),
            short_collection(collection),
            inst,
        );
        Ok((Value::Object(m), text))
    }

    async fn series_window(&self, p: &ShowSeriesParams, wanted: &[&str]) -> anyhow::Result<(String, String, Series, usize, u32)> {
        let collection = p.collection_id.clone().unwrap_or_else(|| DEFAULT_COLLECTION.to_string());
        let hours = p.hours.unwrap_or(24).clamp(1, 48);
        let (inst, series) = self
            .point_series(&collection, p.instance_id.as_deref(), p.latitude, p.longitude, wanted)
            .await?;
        let start = shaping::now_index(&series.times, series.reference_time.as_deref(), Utc::now());
        Ok((collection, inst, series, start, hours))
    }

    async fn forecast_impl(&self, p: ShowSeriesParams) -> ToolOutput {
        let (collection, inst, series, start, hours) = self.series_window(&p, shaping::STEP_PARAMS).await?;
        let steps = step_window(&shaping::build_steps(&series), start, hours);
        let summary = shaping::summarize(&steps);
        let mut m = envelope(
            "forecast",
            location_json(p.location_name.as_deref(), p.latitude, p.longitude),
            model_json(&collection, &inst, series.reference_time.as_deref()),
            Some(&collection),
        );
        m.insert("steps".into(), json!(steps));
        m.insert("summary".into(), json!(summary));
        let text = format!(
            "Vorhersage {} für {} Stunden ab {} (Ortszeit): Temperatur {}…{} °C, Niederschlag Σ {} mm, \
             max. Böe {} km/h ({}), mittlere Bewölkung {} %. Modelllauf {} {}.",
            place(p.location_name.as_deref(), p.latitude, p.longitude),
            steps.len().saturating_sub(1),
            steps.first().map(|s| local_time(&s.time)).unwrap_or_default(),
            num(summary.t_min_c, 1),
            num(summary.t_max_c, 1),
            num(summary.precip_sum_mm, 1),
            num(summary.gust_max_kmh, 0),
            summary.gust_max_time.as_deref().map(local_time).unwrap_or("–".into()),
            summary.cloud_mean_pct.map(|c| c.to_string()).unwrap_or("–".into()),
            short_collection(&collection),
            inst,
        );
        Ok((Value::Object(m), text))
    }

    async fn wind_impl(&self, p: ShowSeriesParams) -> ToolOutput {
        let (collection, inst, series, start, hours) = self.series_window(&p, shaping::STEP_PARAMS).await?;
        let steps = step_window(&shaping::build_steps(&series), start, hours);
        let summary = shaping::wind_summary(&steps);
        let mut m = envelope(
            "wind",
            location_json(p.location_name.as_deref(), p.latitude, p.longitude),
            model_json(&collection, &inst, series.reference_time.as_deref()),
            Some(&collection),
        );
        m.insert("steps".into(), json!(steps));
        m.insert("summary".into(), json!(summary));
        let text = format!(
            "Wind {} ({} h): Mittel {} km/h, max. {} km/h, max. Böe {} km/h ({}), vorherrschend aus {}.",
            place(p.location_name.as_deref(), p.latitude, p.longitude),
            steps.len().saturating_sub(1),
            num(summary.mean_speed_kmh, 0),
            num(summary.max_speed_kmh, 0),
            num(summary.max_gust_kmh, 0),
            summary.max_gust_time.as_deref().map(local_time).unwrap_or("–".into()),
            summary
                .prevailing_dir_deg
                .map(|d| format!("{} ({d}°)", shaping::compass_de(d as f64)))
                .unwrap_or("–".into()),
        );
        Ok((Value::Object(m), text))
    }

    async fn storm_impl(&self, p: ShowSeriesParams) -> ToolOutput {
        let (collection, inst, series, start, hours) = self.series_window(&p, shaping::STORM_PARAMS).await?;
        let steps = step_window(&shaping::build_storm_steps(&series), start, hours);
        let summary = shaping::storm_summary(&steps);
        let mut m = envelope(
            "storm",
            location_json(p.location_name.as_deref(), p.latitude, p.longitude),
            model_json(&collection, &inst, series.reference_time.as_deref()),
            Some(&collection),
        );
        m.insert("steps".into(), json!(steps));
        m.insert("summary".into(), json!(summary));
        m.insert("levels".into(), shaping::storm_levels());
        let text = format!(
            "Gewitterrisiko {} ({} h): höchste Stufe {} ({}){}; max. CAPE {} J/kg, max. LPI {}, max. Böe {} km/h, \
             Gewitterstunden: {}.",
            place(p.location_name.as_deref(), p.latitude, p.longitude),
            steps.len().saturating_sub(1),
            summary.max_risk,
            shaping::STORM_LEVEL_LABELS[summary.max_risk as usize],
            summary.max_risk_time.as_deref().map(|t| format!(" um {}", local_time(t))).unwrap_or_default(),
            num(summary.max_cape_jkg, 0),
            num(summary.max_lpi, 1),
            num(summary.max_gust_kmh, 0),
            summary.thunder_hours,
        );
        Ok((Value::Object(m), text))
    }

    async fn compare_impl(&self, p: ShowCompareParams) -> ToolOutput {
        if !(2..=6).contains(&p.locations.len()) {
            anyhow::bail!("locations must contain 2 to 6 places (got {})", p.locations.len());
        }
        for l in &p.locations {
            validate_point(l.latitude, l.longitude)?;
        }
        let collection = p.collection_id.clone().unwrap_or_else(|| DEFAULT_COLLECTION.to_string());
        let hours = p.hours.unwrap_or(24).clamp(1, 48);
        // Every location uses the same run; if any of them fails on the newest run,
        // the whole comparison is retried on the next older one.
        let (inst, all_series) = self
            .with_instance(&collection, None, |inst| {
                let collection = collection.clone();
                let locations = &p.locations;
                async move {
                    let results = join_all(locations.iter().map(|l| {
                        self.point_series(&collection, Some(&inst), l.latitude, l.longitude, shaping::STEP_PARAMS)
                    }))
                    .await;
                    locations
                        .iter()
                        .zip(results)
                        .map(|(l, r)| {
                            r.map(|(_, s)| s).map_err(|e| {
                                e.context(format!("location '{}'", place(l.name.as_deref(), l.latitude, l.longitude)))
                            })
                        })
                        .collect::<anyhow::Result<Vec<Series>>>()
                }
            })
            .await?;

        let now = Utc::now();
        let mut locs = Vec::new();
        let mut lines = Vec::new();
        let mut reference = None;
        for (l, series) in p.locations.iter().zip(all_series) {
            let name = place(l.name.as_deref(), l.latitude, l.longitude);
            reference = reference.or(series.reference_time.clone());
            let start = shaping::now_index(&series.times, series.reference_time.as_deref(), now);
            let steps = step_window(&shaping::build_steps(&series), start, hours);
            let summary = shaping::summarize(&steps);
            lines.push(format!(
                "{name}: {}…{} °C, Σ {} mm, Böen bis {} km/h, Bewölkung Ø {} %",
                num(summary.t_min_c, 1),
                num(summary.t_max_c, 1),
                num(summary.precip_sum_mm, 1),
                num(summary.gust_max_kmh, 0),
                summary.cloud_mean_pct.map(|c| c.to_string()).unwrap_or("–".into()),
            ));
            locs.push(json!({
                "name": l.name,
                "lat": l.latitude,
                "lon": l.longitude,
                "steps": steps,
                "summary": summary,
            }));
        }
        let mut m = envelope("compare", Value::Null, model_json(&collection, &inst, reference.as_deref()), Some(&collection));
        m.insert("locations".into(), Value::Array(locs));
        let text = format!("Ortsvergleich, nächste {hours} h (Modelllauf {inst}):\n{}", lines.join("\n"));
        Ok((Value::Object(m), text))
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_area(&self, collection: &str, inst: &str, bbox: BBox, lons: &[f64], lats: &[f64], param: &str, datetime: &str) -> anyhow::Result<Coverage> {
        // The cube endpoint is limited to 10 000 km²; larger areas are sampled
        // with MULTIPOINT position queries at the grid points (chunked for URL length).
        if bbox.area_km2() <= 9_000.0 {
            let raw = self
                .client
                .get_cube_raw(collection, &bbox.to_param(), Some(inst), Some(param), Some(datetime), None, "CoverageJSON")
                .await?;
            return shaping::parse_coverage(&raw);
        }
        let points: Vec<(f64, f64)> = lats.iter().flat_map(|&y| lons.iter().map(move |&x| (x, y))).collect();
        let chunks: Vec<&[(f64, f64)]> = points.chunks(700).collect();
        let parts = join_all(
            chunks
                .iter()
                .map(|c| self.client.get_multipoint_raw(collection, inst, c, param, Some(datetime))),
        )
        .await;
        let mut merged = Coverage::default();
        for part in parts {
            let part = match part {
                Ok(raw) => match shaping::parse_coverage(&raw) {
                    Ok(c) => c,
                    Err(_) => continue, // chunk entirely outside the model domain
                },
                Err(e) => return Err(e),
            };
            if !merged.times.is_empty() && merged.times != part.times {
                anyhow::bail!("inconsistent time axes between area chunks");
            }
            merged = shaping::merge_coverages(merged, part);
        }
        if merged.times.is_empty() {
            anyhow::bail!("no model data in this area (outside the ICON-D2 domain?)");
        }
        Ok(merged)
    }

    async fn area_map_impl(&self, p: ShowAreaMapParams) -> ToolOutput {
        let collection = DEFAULT_COLLECTION;
        let pname = p.parameter.as_deref().unwrap_or("T_2M");
        let param = shaping::area_param(pname).ok_or_else(|| {
            anyhow::anyhow!("parameter must be one of T_2M, TOT_PREC, CLCT, VMAX_10M, CAPE_ML (got '{pname}')")
        })?;
        let offered = self.client.collection_param_names(collection).await?;
        let available: Vec<shaping::AreaParam> = shaping::AREA_PARAMS
            .iter()
            .copied()
            .filter(|a| offered.iter().any(|n| n == a.name))
            .collect();
        if !available.iter().any(|a| a.name == param.name) {
            anyhow::bail!("parameter {} is not offered by {collection}", param.name);
        }

        let center = match (p.latitude, p.longitude) {
            (Some(lat), Some(lon)) => {
                validate_point(lat, lon)?;
                Some((lat, lon))
            }
            _ => None,
        };
        let bbox = match (&p.bbox, center) {
            (Some(b), _) => shaping::parse_bbox(b)?,
            (None, Some((lat, lon))) => {
                let r = p.radius_km.unwrap_or(60.0);
                if !r.is_finite() || r <= 0.0 {
                    anyhow::bail!("radius_km must be > 0");
                }
                shaping::bbox_around(lat, lon, r.clamp(2.0, 250.0))
            }
            (None, None) => anyhow::bail!("give either bbox or latitude+longitude (with optional radius_km)"),
        };
        if bbox.area_km2() > 520.0 * 520.0 {
            anyhow::bail!("area too large (max about 500 × 500 km)");
        }
        let hours = p.hours.unwrap_or(12).clamp(1, 24);
        let candidates = self.client.instance_candidates(collection).await?;

        let (inst, (cov, start_idx, grid)) = self
            .with_instance(collection, None, |inst| {
                let info = candidates.iter().find(|c| c.id == inst).cloned();
                async move {
                    let reference = shaping::parse_time(&inst).ok_or_else(|| anyhow::anyhow!("bad instance id {inst}"))?;
                    let run_end = info
                        .and_then(|i| i.end)
                        .and_then(|e| shaping::parse_time(&e))
                        .unwrap_or(reference + Duration::hours(27));
                    let max_k = (run_end - reference).num_hours().max(0);
                    let k = ((Utc::now() - reference).num_minutes() as f64 / 60.0).round() as i64;
                    let k = k.clamp(0, max_k);
                    let start = reference + Duration::hours(k);
                    let end = (start + Duration::hours(hours as i64)).min(run_end);
                    let fetch_start = if param.name == "TOT_PREC" && k > 0 { start - Duration::hours(1) } else { start };
                    let n_times = ((end - start).num_hours() + 1).max(1) as usize;
                    // keep the payload small: ≤ ~48k grid values overall
                    let (w, h) = shaping::grid_dims(bbox, (48_000 / n_times).clamp(64, 4096));
                    let (lons, lats) = shaping::grid_axes(bbox, w, h);
                    let datetime = format!("{}/{}", shaping::iso(fetch_start), shaping::iso(end));
                    let cov = self.fetch_area(collection, &inst, bbox, &lons, &lats, param.name, &datetime).await?;
                    let skip = usize::from(fetch_start < start);
                    Ok((cov, skip, (lons, lats)))
                }
            })
            .await?;
        let (lons, lats) = grid;

        let reference = cov.reference_time.clone().unwrap_or_else(|| inst.clone());
        let raw = cov
            .ranges
            .get(param.name)
            .ok_or_else(|| anyhow::anyhow!("DWD response lacks {}", param.name))?;
        let ncell = cov.polygons.len();
        let cell_series: Vec<Vec<Option<f64>>> = (0..ncell)
            .map(|c| {
                let raw_c: Vec<Option<f64>> = raw.iter().map(|row| row.get(c).copied().flatten()).collect();
                shaping::convert_area_series(param.name, &raw_c, &cov.times, Some(&reference))
            })
            .collect();
        let mapping = shaping::map_grid_to_cells(&cov.polygons, &lons, &lats);
        let times: Vec<String> = cov.times.iter().skip(start_idx).cloned().collect();
        let (w, h) = (lons.len(), lats.len());
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let values: Vec<Vec<Vec<Option<f64>>>> = (0..times.len())
            .map(|t| {
                (0..h)
                    .map(|y| {
                        (0..w)
                            .map(|x| {
                                let v = mapping[y * w + x].and_then(|c| cell_series[c].get(t + start_idx).copied().flatten());
                                if let Some(v) = v {
                                    min = min.min(v);
                                    max = max.max(v);
                                }
                                v
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();
        if !min.is_finite() {
            anyhow::bail!("no {} values in this area (outside the ICON-D2 domain?)", param.name);
        }

        let (loc, markers) = match center {
            Some((lat, lon)) => (
                location_json(p.location_name.as_deref(), lat, lon),
                json!([{ "name": p.location_name, "lat": lat, "lon": lon }]),
            ),
            None => {
                let (cx, cy) = bbox.center();
                (location_json(p.location_name.as_deref(), shaping::round_to(cy, 4), shaping::round_to(cx, 4)), json!([]))
            }
        };
        let mut m = envelope("area-map", loc, model_json(collection, &inst, Some(&reference)), Some(collection));
        m.insert("bbox".into(), json!(bbox.to_vec()));
        m.insert("parameter".into(), json!(param));
        m.insert("available_parameters".into(), json!(available));
        m.insert("times".into(), json!(times));
        m.insert("lons".into(), json!(lons));
        m.insert("lats".into(), json!(lats));
        m.insert("values".into(), json!(values));
        m.insert("stats".into(), json!({ "min": min, "max": max }));
        m.insert("markers".into(), markers);
        let text = format!(
            "Gebietskarte {} ({}) {}: {} Zeitschritte ab {} (Ortszeit), Gitter {}×{}, Wertebereich {}…{} {}. Modelllauf {}.",
            param.label,
            param.name,
            p.location_name.as_deref().map(|n| format!("um {n}")).unwrap_or_else(|| format!("bbox {}", bbox.to_param())),
            times.len(),
            times.first().map(|t| local_time(t)).unwrap_or_default(),
            w,
            h,
            num(Some(min), 1),
            num(Some(max), 1),
            param.unit,
            inst,
        );
        Ok((Value::Object(m), text))
    }

    async fn model_runs_impl(&self, p: ShowModelRunsParams) -> ToolOutput {
        let limit = p.limit.unwrap_or(24).clamp(1, 500) as usize;
        let collections: Vec<Value> = match &p.collection_id {
            Some(id) => vec![self.client.collection_cached(id).await?],
            None => self.client.list_collections().await?.as_array().cloned().unwrap_or_default(),
        };
        if collections.is_empty() {
            anyhow::bail!("DWD API lists no collections");
        }
        let entries = join_all(collections.iter().map(|c| async move {
            let id = c.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let instances = self.client.instance_candidates(&id).await.unwrap_or_default();
            let latest = match self.client.get_latest_instance_id(&id).await {
                Ok(l) => Some(l),
                Err(_) => instances.first().map(|i| i.id.clone()),
            };
            (id, instances, latest)
        }))
        .await;

        let mut out = Vec::new();
        let mut lines = Vec::new();
        for (c, (id, instances, latest)) in collections.iter().zip(entries) {
            let bbox = c.pointer("/extent/spatial/bbox/0").cloned().unwrap_or(Value::Null);
            let interval = c.pointer("/extent/temporal/interval/0");
            let start = interval.and_then(|i| i.get(0)).and_then(Value::as_str);
            let end = interval.and_then(|i| i.get(1)).and_then(Value::as_str);
            let steps = match (start, end) {
                (Some(s), Some(e)) => shaping::hourly_steps(s, e),
                _ => None,
            };
            let mut parameters: Vec<Value> = c
                .get("parameter_names")
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .map(|(name, meta)| {
                            let desc = meta.get("description").and_then(Value::as_str).unwrap_or(name);
                            json!({
                                "name": name,
                                "label": shaping::param_label_de(name).unwrap_or(desc),
                                "unit": meta.get("unit").map(shaping::unit_short).unwrap_or_default(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            parameters.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            let query_types: Vec<String> = c
                .get("data_queries")
                .and_then(Value::as_object)
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let short = short_collection(&id).to_string();
            let (title, description) = describe_collection(&id, steps);
            lines.push(format!(
                "{id}: neuester Lauf {}, {} Läufe verfügbar, {} Parameter, Vorhersage {} … {}",
                latest.as_deref().unwrap_or("–"),
                instances.len(),
                parameters.len(),
                start.unwrap_or("–"),
                end.unwrap_or("–"),
            ));
            out.push(json!({
                "id": id,
                "title": title.unwrap_or(short),
                "description": description,
                "bbox": bbox,
                "temporal": { "start": start, "end": end, "steps": steps },
                "parameters": parameters,
                "query_types": query_types,
                "latest_instance": latest,
                "instances": instances.iter().take(limit).map(|i| i.id.clone()).collect::<Vec<_>>(),
            }));
        }
        let mut m = envelope("model-runs", Value::Null, Value::Null, None);
        m.insert("collections".into(), Value::Array(out));
        let text = format!("DWD EDR API – {} Kollektion(en):\n{}", lines.len(), lines.join("\n"));
        Ok((Value::Object(m), text))
    }
}

fn describe_collection(id: &str, steps: Option<i64>) -> (Option<String>, String) {
    let horizon = steps.map(|s| format!(", Vorhersage bis +{} h", s - 1)).unwrap_or_default();
    match id {
        "ICON-D2-RUC@single_level" => (
            Some("ICON-D2-RUC".into()),
            format!(
                "Rapid Update Cycle des hochauflösenden Regionalmodells ICON-D2 (ca. 2,2 km, Deutschland und Nachbarländer), \
                 stündlich neu gerechnet{horizon}. Bodennahe Felder (single level)."
            ),
        ),
        "ICON-D2-RUC-EPS@single_level" => (
            Some("ICON-D2-RUC-EPS".into()),
            format!(
                "Ensemble (20 Member) des ICON-D2 Rapid Update Cycle, stündlich neu gerechnet{horizon}. \
                 Punktwerte werden als Ensemble-Mittel dargestellt."
            ),
        ),
        _ => (None, format!("DWD-Modellkollektion {id}{horizon}.")),
    }
}

// -----------------------------------------------------------------------------
// Tools
// -----------------------------------------------------------------------------

#[tool_router(router = show_router, vis = "pub(crate)")]
impl DwdMcpServer {
    #[tool(
        name = "show_current_weather",
        description = "Show the current weather at a place as an interactive card (temperature, wind, gusts, clouds, precipitation now and the next hours) from the DWD ICON-D2-RUC model. Use when the user wants to SEE the current weather.",
        meta = widgets::tool_meta("current"),
        annotations(read_only_hint = true)
    )]
    pub async fn show_current_weather(&self, Parameters(p): Parameters<ShowCurrentParams>) -> CallToolResult {
        finish("show_current_weather", self.current_impl(p).await)
    }

    #[tool(
        name = "show_forecast_chart",
        description = "Show an interactive hourly forecast chart (meteogram: temperature, dew point, precipitation, clouds, wind, gusts, pressure) for a place from DWD ICON-D2-RUC.",
        meta = widgets::tool_meta("forecast"),
        annotations(read_only_hint = true)
    )]
    pub async fn show_forecast_chart(&self, Parameters(p): Parameters<ShowSeriesParams>) -> CallToolResult {
        finish("show_forecast_chart", self.forecast_impl(p).await)
    }

    #[tool(
        name = "show_wind_profile",
        description = "Show an interactive wind chart for a place: wind speed, gusts and direction per hour plus prevailing direction (DWD ICON-D2-RUC).",
        meta = widgets::tool_meta("wind"),
        annotations(read_only_hint = true)
    )]
    pub async fn show_wind_profile(&self, Parameters(p): Parameters<ShowSeriesParams>) -> CallToolResult {
        finish("show_wind_profile", self.wind_impl(p).await)
    }

    #[tool(
        name = "show_thunderstorm_risk",
        description = "Show an interactive thunderstorm / convection outlook for a place: hourly risk level from CAPE, lightning potential (LPI), hail, gusts and weather code (DWD ICON-D2-RUC).",
        meta = widgets::tool_meta("storm"),
        annotations(read_only_hint = true)
    )]
    pub async fn show_thunderstorm_risk(&self, Parameters(p): Parameters<ShowSeriesParams>) -> CallToolResult {
        finish("show_thunderstorm_risk", self.storm_impl(p).await)
    }

    #[tool(
        name = "show_location_compare",
        description = "Show an interactive side-by-side forecast comparison of 2–6 places (temperature, precipitation, wind, clouds) from DWD ICON-D2-RUC.",
        meta = widgets::tool_meta("compare"),
        annotations(read_only_hint = true)
    )]
    pub async fn show_location_compare(&self, Parameters(p): Parameters<ShowCompareParams>) -> CallToolResult {
        finish("show_location_compare", self.compare_impl(p).await)
    }

    #[tool(
        name = "show_area_map",
        description = "Show an interactive gridded weather map over a region with a time slider: temperature, hourly precipitation, cloud cover, gusts or CAPE from DWD ICON-D2-RUC. Give a bbox or a center (latitude/longitude) with radius_km.",
        meta = widgets::tool_meta("area-map"),
        annotations(read_only_hint = true)
    )]
    pub async fn show_area_map(&self, Parameters(p): Parameters<ShowAreaMapParams>) -> CallToolResult {
        finish("show_area_map", self.area_map_impl(p).await)
    }

    #[tool(
        name = "show_model_runs",
        description = "Show an interactive explorer of the DWD open-data model collections: available model runs (instances), forecast range, parameters and query types.",
        meta = widgets::tool_meta("model-runs"),
        annotations(read_only_hint = true)
    )]
    pub async fn show_model_runs(&self, Parameters(p): Parameters<ShowModelRunsParams>) -> CallToolResult {
        finish("show_model_runs", self.model_runs_impl(p).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn berlin_dst() {
        let summer = Utc.with_ymd_and_hms(2026, 10, 9, 12, 0, 0).unwrap();
        assert_eq!(berlin_offset_hours(summer), 2);
        let winter = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
        assert_eq!(berlin_offset_hours(winter), 1);
        assert_eq!(local_time("2026-10-09T09:00:00Z"), "09.10. 11:00");
    }
}
