//! Pure data shaping: unit conversion, CoverageJSON parsing, derived
//! quantities (de-accumulated precipitation, wind speed/direction, storm risk),
//! summaries and the area-map gridding. Everything here is synchronous and
//! unit-tested; the payload shapes follow `widgets/CONTRACT.md`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

// -----------------------------------------------------------------------------
// Basic conversions
// -----------------------------------------------------------------------------

pub fn round_to(v: f64, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    let r = (v * f).round() / f;
    // avoid "-0.0" in JSON
    if r == 0.0 { 0.0 } else { r }
}

fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

/// Kelvin → °C (1 decimal).
pub fn k_to_c(k: f64) -> f64 {
    round_to(k - 273.15, 1)
}

/// Pascal → hPa (1 decimal).
pub fn pa_to_hpa(pa: f64) -> f64 {
    round_to(pa / 100.0, 1)
}

/// m/s → km/h (1 decimal).
pub fn ms_to_kmh(ms: f64) -> f64 {
    round_to(ms * 3.6, 1)
}

/// |(u, v)| in km/h (1 decimal).
pub fn wind_speed_kmh(u: f64, v: f64) -> f64 {
    ms_to_kmh(u.hypot(v))
}

/// Meteorological wind direction (the direction the wind blows *from*), 0..360°.
/// Returns `None` for calm (u = v = 0).
pub fn wind_dir_deg(u: f64, v: f64) -> Option<f64> {
    if u == 0.0 && v == 0.0 {
        return None;
    }
    Some(((-u).atan2(-v).to_degrees() + 360.0) % 360.0)
}

/// Round a direction to an integer degree in 0..=359.
pub fn dir_int(deg: f64) -> i64 {
    (deg.round() as i64).rem_euclid(360)
}

/// German 16-point compass label for a "from" direction.
pub fn compass_de(deg: f64) -> &'static str {
    const P: [&str; 16] = [
        "N", "NNO", "NO", "ONO", "O", "OSO", "SO", "SSO", "S", "SSW", "SW", "WSW", "W", "WNW",
        "NW", "NNW",
    ];
    P[((deg.rem_euclid(360.0) / 22.5).round() as usize) % 16]
}

/// Hourly amounts from an accumulated series (e.g. TOT_PREC since run start).
/// `first_is_run_start`: the first value is at the reference time, so its
/// amount equals the accumulated value itself; otherwise it is unknown.
/// Differences are clamped to ≥ 0 (numerical noise / re-gridding).
pub fn deaccumulate(acc: &[Option<f64>], first_is_run_start: bool) -> Vec<Option<f64>> {
    acc.iter()
        .enumerate()
        .map(|(i, cur)| {
            let cur = finite(*cur)?;
            if i == 0 {
                return first_is_run_start.then_some(cur.max(0.0));
            }
            let prev = finite(acc[i - 1])?;
            Some((cur - prev).max(0.0))
        })
        .collect()
}

pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

pub fn iso(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn same_time(a: &str, b: &str) -> bool {
    match (parse_time(a), parse_time(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Index of the step closest to `now` among steps at/after the reference time.
pub fn now_index(times: &[String], reference: Option<&str>, now: DateTime<Utc>) -> usize {
    let reference = reference.and_then(parse_time);
    times
        .iter()
        .enumerate()
        .filter_map(|(i, t)| parse_time(t).map(|t| (i, t)))
        .filter(|(_, t)| reference.is_none_or(|r| *t >= r))
        .min_by_key(|(_, t)| (*t - now).num_seconds().abs())
        .map(|(i, _)| i)
        .unwrap_or(0)
}

// -----------------------------------------------------------------------------
// CoverageJSON
// -----------------------------------------------------------------------------

/// A parsed CoverageJSON (Multi)PolygonSeries: one value per time step and cell.
/// Extra axes (e.g. the ensemble axis `e`) are averaged.
#[derive(Debug, Default)]
pub struct Coverage {
    pub times: Vec<String>,
    pub reference_time: Option<String>,
    /// Outer ring of each cell polygon, (lon, lat).
    pub polygons: Vec<Vec<(f64, f64)>>,
    /// parameter → values[t][cell]
    pub ranges: HashMap<String, Vec<Vec<Option<f64>>>>,
}

fn parse_single_coverage(cov: &Value) -> anyhow::Result<Coverage> {
    let axes = cov.pointer("/domain/axes");
    let times: Vec<String> = axes
        .and_then(|a| a.pointer("/t/values"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let polygons: Vec<Vec<(f64, f64)>> = axes
        .and_then(|a| a.pointer("/composite/values"))
        .and_then(Value::as_array)
        .map(|cells| {
            cells
                .iter()
                .map(|poly| {
                    // polygon = [ring][point][x,y]; tolerate a bare ring or point.
                    let ring = poly
                        .get(0)
                        .map(|r| if r.get(0).is_some_and(Value::is_array) { r } else { poly })
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    ring.iter()
                        .filter_map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();

    let nt = times.len().max(1);
    let nc = polygons.len().max(1);
    let mut ranges = HashMap::new();

    if let Some(obj) = cov.get("ranges").and_then(Value::as_object) {
        for (name, r) in obj {
            let values = r.get("values").and_then(Value::as_array).cloned().unwrap_or_default();
            let axis_names: Vec<String> = r
                .get("axisNames")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_else(|| vec!["t".into(), "composite".into()]);
            let shape: Vec<usize> = r
                .get("shape")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_u64().map(|x| x as usize)).collect())
                .unwrap_or_else(|| vec![nt, nc]);
            if shape.len() != axis_names.len() || shape.iter().product::<usize>() != values.len() {
                anyhow::bail!("Unexpected CoverageJSON range shape for {name}");
            }
            let t_pos = axis_names.iter().position(|a| a == "t");
            let c_pos = axis_names.iter().position(|a| a == "composite");
            let mut strides = vec![1usize; shape.len()];
            for i in (0..shape.len().saturating_sub(1)).rev() {
                strides[i] = strides[i + 1] * shape[i + 1];
            }
            let mut sum = vec![0f64; nt * nc];
            let mut cnt = vec![0u32; nt * nc];
            for (k, v) in values.iter().enumerate() {
                let Some(x) = v.as_f64().filter(|x| x.is_finite()) else { continue };
                let t = t_pos.map(|p| (k / strides[p]) % shape[p]).unwrap_or(0);
                let c = c_pos.map(|p| (k / strides[p]) % shape[p]).unwrap_or(0);
                if t < nt && c < nc {
                    sum[t * nc + c] += x;
                    cnt[t * nc + c] += 1;
                }
            }
            let grid: Vec<Vec<Option<f64>>> = (0..nt)
                .map(|t| {
                    (0..nc)
                        .map(|c| {
                            let i = t * nc + c;
                            (cnt[i] > 0).then(|| sum[i] / cnt[i] as f64)
                        })
                        .collect()
                })
                .collect();
            ranges.insert(name.clone(), grid);
        }
    }

    Ok(Coverage {
        times,
        reference_time: cov
            .get("cf:forecast_reference_time")
            .and_then(Value::as_str)
            .map(String::from),
        polygons,
        ranges,
    })
}

/// Parse a CoverageCollection (or single Coverage). Multiple coverages with the
/// same time axis are concatenated along the cell axis.
pub fn parse_coverage(v: &Value) -> anyhow::Result<Coverage> {
    let list: Vec<&Value> = match v.get("coverages").and_then(Value::as_array) {
        Some(a) => a.iter().collect(),
        None if v.get("type").and_then(Value::as_str) == Some("Coverage") => vec![v],
        None => Vec::new(),
    };
    let mut out: Option<Coverage> = None;
    for c in list {
        let cov = parse_single_coverage(c)?;
        out = Some(match out {
            None => cov,
            Some(acc) => merge_coverages(acc, cov),
        });
    }
    out.filter(|c| !c.times.is_empty())
        .ok_or_else(|| anyhow::anyhow!("DWD API returned no coverage data"))
}

/// Concatenate cells of two coverages (same time axis assumed).
pub fn merge_coverages(mut a: Coverage, b: Coverage) -> Coverage {
    if a.times.is_empty() {
        return b;
    }
    let (na, nb) = (a.polygons.len(), b.polygons.len());
    let nt = a.times.len();
    let mut names: Vec<String> = a.ranges.keys().chain(b.ranges.keys()).cloned().collect();
    names.sort();
    names.dedup();
    for name in names {
        let ra = a.ranges.remove(&name).unwrap_or_else(|| vec![vec![None; na]; nt]);
        let rb = b.ranges.get(&name).cloned().unwrap_or_else(|| vec![vec![None; nb]; nt]);
        let merged = (0..nt)
            .map(|t| {
                let mut row = ra.get(t).cloned().unwrap_or_else(|| vec![None; na]);
                row.extend(rb.get(t).cloned().unwrap_or_else(|| vec![None; nb]));
                row
            })
            .collect();
        a.ranges.insert(name, merged);
    }
    a.polygons.extend(b.polygons);
    a
}

/// A single-location time series (cell 0 of a position query).
#[derive(Debug, Default, Clone)]
pub struct Series {
    pub times: Vec<String>,
    pub reference_time: Option<String>,
    pub values: HashMap<String, Vec<Option<f64>>>,
}

impl Coverage {
    pub fn point_series(&self) -> Series {
        Series {
            times: self.times.clone(),
            reference_time: self.reference_time.clone(),
            values: self
                .ranges
                .iter()
                .map(|(k, grid)| (k.clone(), grid.iter().map(|row| row.first().copied().flatten()).collect()))
                .collect(),
        }
    }
}

impl Series {
    fn col(&self, name: &str) -> Vec<Option<f64>> {
        self.values
            .get(name)
            .cloned()
            .unwrap_or_else(|| vec![None; self.times.len()])
    }

    fn starts_at_reference(&self) -> bool {
        match (self.times.first(), &self.reference_time) {
            (Some(t), Some(r)) => same_time(t, r),
            _ => false,
        }
    }

    fn is_reference(&self, i: usize) -> bool {
        match &self.reference_time {
            Some(r) => same_time(&self.times[i], r),
            None => false,
        }
    }
}

// -----------------------------------------------------------------------------
// Steps & summaries
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Step {
    pub time: String,
    pub t2m_c: Option<f64>,
    pub td2m_c: Option<f64>,
    pub precip_mm: Option<f64>,
    pub precip_total_mm: Option<f64>,
    pub cloud_pct: Option<i64>,
    pub wind_speed_kmh: Option<f64>,
    pub wind_dir_deg: Option<i64>,
    pub gust_kmh: Option<f64>,
    pub pmsl_hpa: Option<f64>,
    pub ww: Option<i64>,
}

/// Parameters needed to build [`Step`]s.
pub const STEP_PARAMS: &[&str] = &[
    "T_2M", "TD_2M", "TOT_PREC", "CLCT", "U_10M", "V_10M", "VMAX_10M", "PMSL", "WW",
];

pub fn build_steps(s: &Series) -> Vec<Step> {
    let t2m = s.col("T_2M");
    let td = s.col("TD_2M");
    let tp = s.col("TOT_PREC");
    let hourly = deaccumulate(&tp, s.starts_at_reference());
    let clct = s.col("CLCT");
    let u = s.col("U_10M");
    let v = s.col("V_10M");
    let vmax = s.col("VMAX_10M");
    let pmsl = s.col("PMSL");
    let ww = s.col("WW");

    (0..s.times.len())
        .map(|i| {
            let wind = match (finite(u[i]), finite(v[i])) {
                (Some(u), Some(v)) => Some((u, v)),
                _ => None,
            };
            Step {
                time: s.times[i].clone(),
                t2m_c: finite(t2m[i]).map(k_to_c),
                td2m_c: finite(td[i]).map(k_to_c),
                precip_mm: hourly[i].map(|x| round_to(x, 2)),
                precip_total_mm: finite(tp[i]).map(|x| round_to(x.max(0.0), 2)),
                cloud_pct: finite(clct[i]).map(|x| x.round().clamp(0.0, 100.0) as i64),
                wind_speed_kmh: wind.map(|(u, v)| wind_speed_kmh(u, v)),
                wind_dir_deg: wind.and_then(|(u, v)| wind_dir_deg(u, v)).map(dir_int),
                // VMAX_10M is a max over the preceding interval: undefined at lead time 0.
                gust_kmh: if s.is_reference(i) { None } else { finite(vmax[i]).map(ms_to_kmh) },
                pmsl_hpa: finite(pmsl[i]).map(pa_to_hpa),
                ww: finite(ww[i]).map(|x| x.round() as i64),
            }
        })
        .collect()
}

fn max_by_val<T: Copy>(items: impl Iterator<Item = (T, f64)>) -> Option<(T, f64)> {
    items.fold(None, |best, (k, v)| match best {
        Some((_, bv)) if bv >= v => best,
        _ => Some((k, v)),
    })
}

fn mean(vals: impl Iterator<Item = f64>) -> Option<f64> {
    let (s, n) = vals.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| s / n as f64)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    pub t_min_c: Option<f64>,
    pub t_max_c: Option<f64>,
    pub precip_sum_mm: Option<f64>,
    pub gust_max_kmh: Option<f64>,
    pub gust_max_time: Option<String>,
    pub cloud_mean_pct: Option<i64>,
}

pub fn summarize(steps: &[Step]) -> Summary {
    let temps: Vec<f64> = steps.iter().filter_map(|s| s.t2m_c).collect();
    let precip: Vec<f64> = steps.iter().filter_map(|s| s.precip_mm).collect();
    let gust = max_by_val(steps.iter().filter_map(|s| s.gust_kmh.map(|g| (s, g))));
    Summary {
        t_min_c: temps.iter().copied().reduce(f64::min),
        t_max_c: temps.iter().copied().reduce(f64::max),
        precip_sum_mm: (!precip.is_empty()).then(|| round_to(precip.iter().sum(), 2)),
        gust_max_kmh: gust.map(|(_, g)| g),
        gust_max_time: gust.map(|(s, _)| s.time.clone()),
        cloud_mean_pct: mean(steps.iter().filter_map(|s| s.cloud_pct.map(|c| c as f64)))
            .map(|m| m.round() as i64),
    }
}

/// Direction of the speed-weighted mean wind vector (meteorological "from").
pub fn prevailing_dir(steps: &[Step]) -> Option<f64> {
    let (mut su, mut sv, mut n) = (0.0, 0.0, 0usize);
    for s in steps {
        if let (Some(spd), Some(dir)) = (s.wind_speed_kmh, s.wind_dir_deg) {
            let r = (dir as f64).to_radians();
            // "from" direction → vector components (u, v) of the flow
            su += -spd * r.sin();
            sv += -spd * r.cos();
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    let (u, v) = (su / n as f64, sv / n as f64);
    if u.hypot(v) < 1e-6 {
        return None;
    }
    wind_dir_deg(u, v)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WindSummary {
    pub mean_speed_kmh: Option<f64>,
    pub max_speed_kmh: Option<f64>,
    pub max_gust_kmh: Option<f64>,
    pub max_gust_time: Option<String>,
    pub prevailing_dir_deg: Option<i64>,
}

pub fn wind_summary(steps: &[Step]) -> WindSummary {
    let gust = max_by_val(steps.iter().filter_map(|s| s.gust_kmh.map(|g| (s, g))));
    WindSummary {
        mean_speed_kmh: mean(steps.iter().filter_map(|s| s.wind_speed_kmh)).map(|m| round_to(m, 1)),
        max_speed_kmh: steps.iter().filter_map(|s| s.wind_speed_kmh).reduce(f64::max),
        max_gust_kmh: gust.map(|(_, g)| g),
        max_gust_time: gust.map(|(s, _)| s.time.clone()),
        prevailing_dir_deg: prevailing_dir(steps).map(dir_int),
    }
}

// -----------------------------------------------------------------------------
// Thunderstorm risk
// -----------------------------------------------------------------------------

pub const STORM_PARAMS: &[&str] = &["CAPE_ML", "LPI", "HAIL_GSP", "VMAX_10M", "TOT_PREC", "WW"];

/// Risk level 0–3; first matching rule from the top (see CONTRACT.md).
pub fn storm_risk(cape: Option<f64>, lpi: Option<f64>, gust_kmh: Option<f64>, ww: Option<i64>) -> u8 {
    let ge = |v: Option<f64>, t: f64| v.is_some_and(|x| x >= t);
    if ww.is_some_and(|w| w >= 95) || ge(lpi, 4.0) || ge(gust_kmh, 90.0) || ge(cape, 2000.0) {
        3
    } else if ge(lpi, 2.0) || ge(gust_kmh, 65.0) || ge(cape, 1000.0) {
        2
    } else if lpi.is_some_and(|x| x > 0.0) || ge(cape, 300.0) || ge(gust_kmh, 50.0) {
        1
    } else {
        0
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StormStep {
    pub time: String,
    pub cape_jkg: Option<f64>,
    pub lpi: Option<f64>,
    pub hail_mm: Option<f64>,
    pub gust_kmh: Option<f64>,
    pub precip_mm: Option<f64>,
    pub ww: Option<i64>,
    pub risk: u8,
}

pub fn build_storm_steps(s: &Series) -> Vec<StormStep> {
    let start = s.starts_at_reference();
    let cape = s.col("CAPE_ML");
    let lpi = s.col("LPI");
    let hail = deaccumulate(&s.col("HAIL_GSP"), start);
    let vmax = s.col("VMAX_10M");
    let precip = deaccumulate(&s.col("TOT_PREC"), start);
    let ww = s.col("WW");
    (0..s.times.len())
        .map(|i| {
            let cape_jkg = finite(cape[i]).map(|x| round_to(x.max(0.0), 0));
            let lpi_v = finite(lpi[i]).map(|x| round_to(x.max(0.0), 1));
            let gust = if s.is_reference(i) { None } else { finite(vmax[i]).map(ms_to_kmh) };
            let ww_v = finite(ww[i]).map(|x| x.round() as i64);
            StormStep {
                time: s.times[i].clone(),
                cape_jkg,
                lpi: lpi_v,
                hail_mm: hail[i].map(|x| round_to(x, 2)),
                gust_kmh: gust,
                precip_mm: precip[i].map(|x| round_to(x, 2)),
                ww: ww_v,
                risk: storm_risk(cape_jkg, lpi_v, gust, ww_v),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StormSummary {
    pub max_risk: u8,
    pub max_risk_time: Option<String>,
    pub max_cape_jkg: Option<f64>,
    pub max_lpi: Option<f64>,
    pub max_gust_kmh: Option<f64>,
    pub thunder_hours: usize,
}

pub fn storm_summary(steps: &[StormStep]) -> StormSummary {
    let max_risk = steps.iter().map(|s| s.risk).max().unwrap_or(0);
    StormSummary {
        max_risk,
        max_risk_time: (max_risk > 0)
            .then(|| steps.iter().find(|s| s.risk == max_risk).map(|s| s.time.clone()))
            .flatten(),
        max_cape_jkg: steps.iter().filter_map(|s| s.cape_jkg).reduce(f64::max),
        max_lpi: steps.iter().filter_map(|s| s.lpi).reduce(f64::max),
        max_gust_kmh: steps.iter().filter_map(|s| s.gust_kmh).reduce(f64::max),
        thunder_hours: steps
            .iter()
            .filter(|s| s.ww.is_some_and(|w| w >= 95) || s.lpi.is_some_and(|l| l > 0.0))
            .count(),
    }
}

pub const STORM_LEVEL_LABELS: [&str; 4] = ["Keine Gefahr", "Gering", "Erhöht", "Hoch"];

pub fn storm_levels() -> Value {
    Value::Array(
        STORM_LEVEL_LABELS
            .iter()
            .enumerate()
            .map(|(i, l)| json!({ "level": i, "label": l }))
            .collect(),
    )
}

// -----------------------------------------------------------------------------
// Area map: bbox, grid, resampling
// -----------------------------------------------------------------------------

pub const KM_PER_DEG: f64 = 111.32;
/// Native ICON-D2 grid spacing (km); no point in sampling finer.
pub const NATIVE_RES_KM: f64 = 2.2;
pub const MAX_GRID: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub minx: f64,
    pub miny: f64,
    pub maxx: f64,
    pub maxy: f64,
}

impl BBox {
    pub fn to_param(self) -> String {
        format!("{:.4},{:.4},{:.4},{:.4}", self.minx, self.miny, self.maxx, self.maxy)
    }
    pub fn to_vec(self) -> Vec<f64> {
        vec![
            round_to(self.minx, 4),
            round_to(self.miny, 4),
            round_to(self.maxx, 4),
            round_to(self.maxy, 4),
        ]
    }
    pub fn center(self) -> (f64, f64) {
        ((self.minx + self.maxx) / 2.0, (self.miny + self.maxy) / 2.0)
    }
    fn size_km(self) -> (f64, f64) {
        let midlat = ((self.miny + self.maxy) / 2.0).to_radians();
        (
            (self.maxx - self.minx) * KM_PER_DEG * midlat.cos(),
            (self.maxy - self.miny) * KM_PER_DEG,
        )
    }
    pub fn area_km2(self) -> f64 {
        let (w, h) = self.size_km();
        w * h
    }
}

pub fn parse_bbox(s: &str) -> anyhow::Result<BBox> {
    let v: Vec<f64> = s
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| anyhow::anyhow!("bbox must be 'minx,miny,maxx,maxy' (numbers), got '{s}'"))?;
    if v.len() != 4 {
        anyhow::bail!("bbox must have 4 numbers 'minx,miny,maxx,maxy', got '{s}'");
    }
    let b = BBox { minx: v[0], miny: v[1], maxx: v[2], maxy: v[3] };
    if !(b.minx < b.maxx && b.miny < b.maxy) {
        anyhow::bail!("bbox must satisfy minx < maxx and miny < maxy");
    }
    if !(-180.0..=180.0).contains(&b.minx)
        || !(-180.0..=180.0).contains(&b.maxx)
        || !(-90.0..=90.0).contains(&b.miny)
        || !(-90.0..=90.0).contains(&b.maxy)
    {
        anyhow::bail!("bbox coordinates out of range (lon −180..180, lat −90..90)");
    }
    Ok(b)
}

pub fn bbox_around(lat: f64, lon: f64, radius_km: f64) -> BBox {
    let dlat = radius_km / KM_PER_DEG;
    let dlon = radius_km / (KM_PER_DEG * lat.to_radians().cos().max(0.05));
    BBox {
        minx: (lon - dlon).max(-180.0),
        miny: (lat - dlat).max(-90.0),
        maxx: (lon + dlon).min(180.0),
        maxy: (lat + dlat).min(90.0),
    }
}

/// Grid width/height (each 2..=64, W*H ≤ max_cells), roughly square cells in km,
/// never finer than the native model resolution.
pub fn grid_dims(b: BBox, max_cells: usize) -> (usize, usize) {
    let (wk, hk) = b.size_km();
    let max_cells = max_cells.clamp(4, MAX_GRID * MAX_GRID);
    let mut res = NATIVE_RES_KM.max(wk.max(hk) / MAX_GRID as f64);
    loop {
        let w = ((wk / res).ceil() as usize).clamp(2, MAX_GRID);
        let h = ((hk / res).ceil() as usize).clamp(2, MAX_GRID);
        if w * h <= max_cells || (w == 2 && h == 2) {
            return (w, h);
        }
        res *= 1.05;
    }
}

/// Cell-centre axes (ascending), rounded to 4 decimals.
pub fn grid_axes(b: BBox, w: usize, h: usize) -> (Vec<f64>, Vec<f64>) {
    let dx = (b.maxx - b.minx) / w as f64;
    let dy = (b.maxy - b.miny) / h as f64;
    (
        (0..w).map(|i| round_to(b.minx + dx * (i as f64 + 0.5), 4)).collect(),
        (0..h).map(|j| round_to(b.miny + dy * (j as f64 + 0.5), 4)).collect(),
    )
}

fn point_in_ring(x: f64, y: f64, ring: &[(f64, f64)]) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = ring[i];
        let (xj, yj) = ring[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn centroid(ring: &[(f64, f64)]) -> Option<(f64, f64)> {
    // drop the closing point if present
    let pts = if ring.len() > 1 && ring.first() == ring.last() { &ring[..ring.len() - 1] } else { ring };
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as f64;
    Some((pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n))
}

/// For every grid point (row-major, index y*W + x) find the model cell that
/// contains it (or the nearest cell centroid within `max_dist_deg`).
pub fn map_grid_to_cells(polygons: &[Vec<(f64, f64)>], lons: &[f64], lats: &[f64]) -> Vec<Option<usize>> {
    const BUCKET: f64 = 0.05;
    const MAX_DIST_DEG: f64 = 0.05;
    let key = |x: f64, y: f64| ((x / BUCKET).floor() as i64, (y / BUCKET).floor() as i64);
    let cents: Vec<Option<(f64, f64)>> = polygons.iter().map(|p| centroid(p)).collect();
    let mut buckets: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, c) in cents.iter().enumerate() {
        if let Some((x, y)) = c {
            buckets.entry(key(*x, *y)).or_default().push(i);
        }
    }
    let mut out = Vec::with_capacity(lons.len() * lats.len());
    for &y in lats {
        let coslat = y.to_radians().cos();
        for &x in lons {
            let (kx, ky) = key(x, y);
            let mut best: Option<(usize, f64)> = None;
            let mut hit = None;
            'search: for dx in -1..=1 {
                for dy in -1..=1 {
                    let Some(cands) = buckets.get(&(kx + dx, ky + dy)) else { continue };
                    for &i in cands {
                        if point_in_ring(x, y, &polygons[i]) {
                            hit = Some(i);
                            break 'search;
                        }
                        if let Some((cx, cy)) = cents[i] {
                            let d = ((cx - x) * coslat).hypot(cy - y);
                            if best.is_none_or(|(_, bd)| d < bd) {
                                best = Some((i, d));
                            }
                        }
                    }
                }
            }
            out.push(hit.or(best.filter(|(_, d)| *d <= MAX_DIST_DEG).map(|(i, _)| i)));
        }
    }
    out
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AreaParam {
    pub name: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub scale: &'static str,
}

pub const AREA_PARAMS: [AreaParam; 5] = [
    AreaParam { name: "T_2M", label: "Temperatur 2 m", unit: "°C", scale: "temp" },
    AreaParam { name: "TOT_PREC", label: "Niederschlag (1 h)", unit: "mm", scale: "precip" },
    AreaParam { name: "CLCT", label: "Bewölkung", unit: "%", scale: "cloud" },
    AreaParam { name: "VMAX_10M", label: "Windböen 10 m", unit: "km/h", scale: "wind" },
    AreaParam { name: "CAPE_ML", label: "CAPE (Mixed Layer)", unit: "J/kg", scale: "cape" },
];

pub fn area_param(name: &str) -> Option<AreaParam> {
    AREA_PARAMS.iter().copied().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// Convert one cell's raw time series to display units.
/// `times`/`reference` are used to blank VMAX_10M at lead time 0 and to decide
/// whether TOT_PREC's first value is at the run start.
pub fn convert_area_series(param: &str, raw: &[Option<f64>], times: &[String], reference: Option<&str>) -> Vec<Option<f64>> {
    let is_ref = |i: usize| reference.is_some_and(|r| times.get(i).is_some_and(|t| same_time(t, r)));
    match param {
        "T_2M" => raw.iter().map(|v| finite(*v).map(k_to_c)).collect(),
        "TOT_PREC" => deaccumulate(raw, is_ref(0)).into_iter().map(|v| v.map(|x| round_to(x, 2))).collect(),
        "CLCT" => raw.iter().map(|v| finite(*v).map(|x| x.round().clamp(0.0, 100.0))).collect(),
        "VMAX_10M" => raw
            .iter()
            .enumerate()
            .map(|(i, v)| if is_ref(i) { None } else { finite(*v).map(ms_to_kmh) })
            .collect(),
        "CAPE_ML" => raw.iter().map(|v| finite(*v).map(|x| round_to(x.max(0.0), 0))).collect(),
        _ => raw.iter().map(|v| finite(*v).map(|x| round_to(x, 2))).collect(),
    }
}

// -----------------------------------------------------------------------------
// Collection metadata
// -----------------------------------------------------------------------------

/// German display label for a DWD parameter (falls back to the API description).
pub fn param_label_de(name: &str) -> Option<&'static str> {
    Some(match name {
        "ASWDIFD_S" => "Diffuse Sonneneinstrahlung",
        "ASWDIR_S" => "Direkte Sonneneinstrahlung",
        "CAPE_ML" => "CAPE (Mixed Layer)",
        "CAPE_MU" => "CAPE (Most Unstable)",
        "CEILING" => "Wolkenuntergrenze",
        "CIN_ML" => "CIN (Mixed Layer)",
        "CIN_MU" => "CIN (Most Unstable)",
        "LPI" => "Blitzpotenzial-Index (LPI)",
        "LPI_MAX" => "Max. Blitzpotenzial-Index",
        "CLCL" => "Tiefe Bewölkung",
        "CLCM" => "Mittelhohe Bewölkung",
        "CLCT" => "Gesamtbewölkung",
        "GRAU_GSP" => "Graupel (akkumuliert)",
        "HAIL_GSP" => "Hagel (akkumuliert)",
        "HSURF" => "Geländehöhe",
        "PMSL" => "Luftdruck (Meeresniveau)",
        "LAI" => "Blattflächenindex",
        "SNOW_GSP" => "Schneefall (akkumuliert)",
        "RAIN_GSP" => "Regen (akkumuliert)",
        "TD_2M" => "Taupunkt 2 m",
        "TOT_PREC" => "Niederschlag (akkumuliert)",
        "T_2M" => "Temperatur 2 m",
        "U_10M" => "Wind U-Komponente 10 m",
        "V_10M" => "Wind V-Komponente 10 m",
        "VMAX_10M" => "Windböen 10 m",
        "WW" => "Signifikantes Wetter (WW)",
        _ => return None,
    })
}

/// Short unit symbol from the API's unit description.
pub fn unit_short(unit: &Value) -> String {
    let sym = unit.pointer("/symbol/value").and_then(Value::as_str).unwrap_or("");
    let label = unit.get("label").and_then(Value::as_str).unwrap_or("");
    match sym {
        "kelvin" => "K".into(),
        "pascal" => "Pa".into(),
        "percent" => "%".into(),
        "meter / second" => "m/s".into(),
        "joule / kilogram" => "J/kg".into(),
        "kilogram / meter ** 2" => "kg/m²".into(),
        "watt / meter ** 2" => "W/m²".into(),
        "meter" => "m".into(),
        "Numeric" => "1".into(),
        "" => label.to_string(),
        other => other.replace(" ** 2", "²").replace(" / ", "/"),
    }
}

/// Number of hourly steps covered by an [start, end] interval (inclusive).
pub fn hourly_steps(start: &str, end: &str) -> Option<i64> {
    let (s, e) = (parse_time(start)?, parse_time(end)?);
    Some((e - s).num_hours() + 1)
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn conversions() {
        assert!(approx(k_to_c(273.15), 0.0));
        assert!(approx(k_to_c(285.55), 12.4));
        assert!(approx(pa_to_hpa(101280.0), 1012.8));
        assert!(approx(ms_to_kmh(10.0), 36.0));
        assert!(approx(wind_speed_kmh(3.0, 4.0), 18.0));
        assert_eq!(round_to(-0.04, 1).to_string(), "0");
    }

    #[test]
    fn wind_direction_meteorological() {
        // wind from the north blows southwards: v < 0
        assert!(approx(wind_dir_deg(0.0, -5.0).unwrap(), 0.0));
        // wind from the east blows westwards: u < 0
        assert!(approx(wind_dir_deg(-5.0, 0.0).unwrap(), 90.0));
        assert!(approx(wind_dir_deg(0.0, 5.0).unwrap(), 180.0));
        assert!(approx(wind_dir_deg(5.0, 0.0).unwrap(), 270.0));
        assert!(approx(wind_dir_deg(3.0, 3.0).unwrap(), 225.0)); // from SW
        assert!(wind_dir_deg(0.0, 0.0).is_none());
        assert_eq!(dir_int(359.6), 0);
        assert_eq!(compass_de(225.0), "SW");
        assert_eq!(compass_de(359.0), "N");
        assert_eq!(compass_de(90.0), "O");
    }

    #[test]
    fn deaccumulation() {
        let acc = [Some(0.0), Some(0.5), Some(1.2), Some(1.1), None, Some(2.0)];
        let h = deaccumulate(&acc, true);
        assert_eq!(h[0], Some(0.0));
        assert!(approx(h[1].unwrap(), 0.5));
        assert!(approx(h[2].unwrap(), 0.7));
        assert_eq!(h[3], Some(0.0)); // clamped
        assert_eq!(h[4], None);
        assert_eq!(h[5], None); // previous unknown
        let h = deaccumulate(&[Some(3.0), Some(4.0)], false);
        assert_eq!(h, vec![None, Some(1.0)]);
    }

    #[test]
    fn risk_rules() {
        assert_eq!(storm_risk(None, None, None, None), 0);
        assert_eq!(storm_risk(Some(299.0), Some(0.0), Some(49.9), Some(61)), 0);
        assert_eq!(storm_risk(Some(300.0), None, None, None), 1);
        assert_eq!(storm_risk(None, Some(0.1), None, None), 1);
        assert_eq!(storm_risk(None, None, Some(50.0), None), 1);
        assert_eq!(storm_risk(Some(1000.0), None, None, None), 2);
        assert_eq!(storm_risk(None, Some(2.0), None, None), 2);
        assert_eq!(storm_risk(None, None, Some(65.0), None), 2);
        assert_eq!(storm_risk(None, None, None, Some(95)), 3);
        assert_eq!(storm_risk(None, Some(4.0), None, None), 3);
        assert_eq!(storm_risk(None, None, Some(90.0), None), 3);
        assert_eq!(storm_risk(Some(2000.0), None, None, Some(3)), 3);
    }

    #[test]
    fn storm_summary_counts_thunder_hours() {
        let mk = |t: &str, lpi: f64, ww: i64| StormStep {
            time: t.into(),
            cape_jkg: Some(100.0),
            lpi: Some(lpi),
            hail_mm: None,
            gust_kmh: Some(20.0),
            precip_mm: Some(0.0),
            ww: Some(ww),
            risk: storm_risk(Some(100.0), Some(lpi), Some(20.0), Some(ww)),
        };
        let steps = vec![mk("a", 0.0, 3), mk("b", 1.0, 61), mk("c", 0.0, 95), mk("d", 0.0, 2)];
        let s = storm_summary(&steps);
        assert_eq!(s.thunder_hours, 2);
        assert_eq!(s.max_risk, 3);
        assert_eq!(s.max_risk_time.as_deref(), Some("c"));
    }

    #[test]
    fn prevailing_direction_is_vector_mean() {
        let mk = |spd: f64, dir: i64| Step {
            time: String::new(),
            t2m_c: None,
            td2m_c: None,
            precip_mm: None,
            precip_total_mm: None,
            cloud_pct: None,
            wind_speed_kmh: Some(spd),
            wind_dir_deg: Some(dir),
            gust_kmh: None,
            pmsl_hpa: None,
            ww: None,
        };
        // 350° and 10° average to north, not 180°
        let d = prevailing_dir(&[mk(10.0, 350), mk(10.0, 10)]).unwrap();
        assert!(d < 0.5 || d > 359.5, "{d}");
        // stronger west wind dominates a weak east wind
        let d = prevailing_dir(&[mk(30.0, 270), mk(5.0, 90)]).unwrap();
        assert!((d - 270.0).abs() < 0.5, "{d}");
    }

    #[test]
    fn grid_dims_are_capped() {
        let b = bbox_around(52.52, 13.405, 80.0);
        let (w, h) = grid_dims(b, 4096);
        assert!(w <= 64 && h <= 64);
        assert!(w >= 60 && h >= 60, "{w}x{h}"); // 160 km / 64 ≈ 2.5 km > native
        let (w, h) = grid_dims(b, 2000);
        assert!(w * h <= 2000 && w <= 64 && h <= 64, "{w}x{h}");
        // small area: never finer than native resolution
        let small = BBox { minx: 13.0, miny: 52.3, maxx: 13.8, maxy: 52.7 };
        let (w, h) = grid_dims(small, 4096);
        assert!(w <= 25 && h <= 21, "{w}x{h}");
        let big = bbox_around(51.0, 10.0, 250.0);
        let (w, h) = grid_dims(big, 4096);
        assert_eq!((w, h), (64, 64));
        let (lons, lats) = grid_axes(big, w, h);
        assert_eq!(lons.len(), 64);
        assert!(lons.windows(2).all(|p| p[0] < p[1]));
        assert!(lats.windows(2).all(|p| p[0] < p[1]));
    }

    #[test]
    fn grid_mapping_point_in_triangle() {
        // two ICON-sized triangles splitting the square 10..10.03 / 50..50.03
        let polys = vec![
            vec![(10.0, 50.0), (10.03, 50.0), (10.0, 50.03), (10.0, 50.0)],
            vec![(10.03, 50.0), (10.03, 50.03), (10.0, 50.03), (10.03, 50.0)],
        ];
        let m = map_grid_to_cells(&polys, &[10.004, 10.02], &[50.004, 50.02]);
        assert_eq!(m, vec![Some(0), Some(0), Some(0), Some(1)]);
        // far away → none
        let m = map_grid_to_cells(&polys, &[20.0], &[50.0]);
        assert_eq!(m, vec![None]);
    }

    #[test]
    fn parse_coverage_with_ensemble_axis() {
        let v = json!({
            "type": "CoverageCollection",
            "coverages": [{
                "type": "Coverage",
                "cf:forecast_reference_time": "2026-10-09T08:00:00Z",
                "domain": { "axes": {
                    "t": { "values": ["2026-10-09T08:00:00Z", "2026-10-09T09:00:00Z"] },
                    "composite": { "values": [[[[0.0,0.0],[1.0,0.0],[0.0,1.0],[0.0,0.0]]]] },
                    "e": { "values": [1, 2] }
                }},
                "ranges": {
                    "T_2M": { "axisNames": ["t","e","composite"], "shape": [2,2,1],
                              "values": [280.0, 282.0, 290.0, null] }
                }
            }]
        });
        let c = parse_coverage(&v).unwrap();
        let s = c.point_series();
        assert_eq!(s.values["T_2M"], vec![Some(281.0), Some(290.0)]);
        assert_eq!(c.polygons[0].len(), 4);
    }

    #[test]
    fn merging_three_chunks_keeps_cells_aligned() {
        let mk = |x: f64, vals: [f64; 2]| Coverage {
            times: vec!["t0".into(), "t1".into()],
            reference_time: None,
            polygons: vec![vec![(x, 0.0)]],
            ranges: HashMap::from([("T_2M".to_string(), vec![vec![Some(vals[0])], vec![Some(vals[1])]])]),
        };
        let m = [mk(1.0, [1.0, 10.0]), mk(2.0, [2.0, 20.0]), mk(3.0, [3.0, 30.0])]
            .into_iter()
            .fold(Coverage::default(), merge_coverages);
        assert_eq!(m.polygons.len(), 3);
        assert_eq!(m.ranges["T_2M"][0], vec![Some(1.0), Some(2.0), Some(3.0)]);
        assert_eq!(m.ranges["T_2M"][1], vec![Some(10.0), Some(20.0), Some(30.0)]);
    }

    #[test]
    fn steps_from_series() {
        let mut values = HashMap::new();
        values.insert("T_2M".into(), vec![Some(283.15), Some(284.15)]);
        values.insert("TOT_PREC".into(), vec![Some(0.0), Some(0.4)]);
        values.insert("U_10M".into(), vec![Some(0.0), Some(-5.0)]);
        values.insert("V_10M".into(), vec![Some(-5.0), Some(0.0)]);
        values.insert("VMAX_10M".into(), vec![Some(0.0), Some(10.0)]);
        values.insert("PMSL".into(), vec![Some(101300.0), None]);
        values.insert("WW".into(), vec![None, Some(61.0)]);
        let s = Series {
            times: vec!["2026-10-09T08:00:00Z".into(), "2026-10-09T09:00:00Z".into()],
            reference_time: Some("2026-10-09T08:00:00Z".into()),
            values,
        };
        let st = build_steps(&s);
        assert_eq!(st[0].t2m_c, Some(10.0));
        assert_eq!(st[0].wind_dir_deg, Some(0));
        assert_eq!(st[1].wind_dir_deg, Some(90));
        assert_eq!(st[0].gust_kmh, None); // lead time 0
        assert_eq!(st[1].gust_kmh, Some(36.0));
        assert_eq!(st[1].precip_mm, Some(0.4));
        assert_eq!(st[0].pmsl_hpa, Some(1013.0));
        assert_eq!(st[1].ww, Some(61));
        assert_eq!(st[0].td2m_c, None);
        let sm = summarize(&st);
        assert_eq!(sm.t_min_c, Some(10.0));
        assert_eq!(sm.t_max_c, Some(11.0));
        assert_eq!(sm.precip_sum_mm, Some(0.4));
        assert_eq!(sm.gust_max_time.as_deref(), Some("2026-10-09T09:00:00Z"));
    }

    #[test]
    fn now_index_picks_closest_after_reference() {
        let times: Vec<String> = (8..14).map(|h| format!("2026-10-09T{h:02}:00:00Z")).collect();
        let now = parse_time("2026-10-09T10:40:00Z").unwrap();
        assert_eq!(now_index(&times, Some("2026-10-09T08:00:00Z"), now), 3);
        let early = parse_time("2026-10-09T05:00:00Z").unwrap();
        assert_eq!(now_index(&times, Some("2026-10-09T09:00:00Z"), early), 1);
    }

    #[test]
    fn bbox_parsing() {
        let b = parse_bbox("13.0, 52.3,13.8,52.7").unwrap();
        assert!(approx(b.maxx, 13.8));
        assert!(parse_bbox("1,2,3").is_err());
        assert!(parse_bbox("14,52,13,53").is_err());
        let b = bbox_around(52.52, 13.405, 80.0);
        assert!((b.area_km2() - 25600.0).abs() < 200.0);
    }

    #[test]
    fn bbox_around_stays_in_bounds() {
        for (lat, lon) in [(0.0, 180.0), (0.0, -180.0), (90.0, 0.0), (-90.0, 0.0)] {
            let b = bbox_around(lat, lon, 250.0);
            assert!(b.minx >= -180.0 && b.maxx <= 180.0 && b.miny >= -90.0 && b.maxy <= 90.0);
            assert!(b.minx < b.maxx && b.miny < b.maxy);
            let s = format!("{},{},{},{}", b.minx, b.miny, b.maxx, b.maxy);
            assert!(parse_bbox(&s).is_ok(), "{s}");
        }
    }
}
