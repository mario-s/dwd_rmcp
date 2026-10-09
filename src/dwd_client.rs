use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::{json, Value};

pub const DWD_BASE_URL: &str =
    "https://nwp.opendata-api.dwd.de/v1beta1";

pub const DEFAULT_COLLECTION: &str =
    "ICON-D2-RUC@single_level";

/// Non-success HTTP response from the DWD API.
#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub body: String,
}

impl HttpError {
    /// Human readable message (uses the problem+json `detail` if present).
    pub fn detail(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|v| {
                v.get("detail")
                    .or_else(|| v.get("title"))
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .unwrap_or_else(|| self.body.chars().take(300).collect())
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DWD API returned HTTP {}\nServer response: {}",
            self.status, self.body
        )
    }
}

impl std::error::Error for HttpError {}

/// True if the error is a 4xx response other than 404 (i.e. a bad request
/// that will fail the same way for any model run).
pub fn is_client_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<HttpError>()
        .is_some_and(|e| (400..500).contains(&e.status) && e.status != 404)
}

/// One forecast run (instance) of a collection.
#[derive(Debug, Clone)]
pub struct InstanceInfo {
    pub id: String,
    pub end: Option<String>,
}

const INSTANCE_TTL: Duration = Duration::from_secs(120);
const COLLECTION_TTL: Duration = Duration::from_secs(600);

type Cache<T> = Arc<Mutex<HashMap<String, (Instant, T)>>>;

fn cache_get<T: Clone>(cache: &Cache<T>, key: &str, ttl: Duration) -> Option<T> {
    let guard = cache.lock().ok()?;
    guard
        .get(key)
        .filter(|(at, _)| at.elapsed() < ttl)
        .map(|(_, v)| v.clone())
}

fn cache_put<T>(cache: &Cache<T>, key: &str, value: T) {
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key.to_string(), (Instant::now(), value));
    }
}

#[derive(Clone)]
pub struct DwdEdrClient {
    client: Client,
    base_url: String,
    instances_cache: Cache<Vec<InstanceInfo>>,
    latest_cache: Cache<String>,
    collection_cache: Cache<Value>,
}

impl DwdEdrClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
            base_url: DWD_BASE_URL.trim_end_matches('/').to_string(),
            instances_cache: Arc::default(),
            latest_cache: Arc::default(),
            collection_cache: Arc::default(),
        }
    }

    async fn get(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> anyhow::Result<Value> {
        let url = format!("{}{}", self.base_url, path);

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .query(params)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            return Err(HttpError {
                status: status.as_u16(),
                body,
            }
            .into());
        }

        Ok(serde_json::from_str(&body)?)
    }

    pub async fn get_landing_page(&self) -> anyhow::Result<Value> {
        self.get("/", &[]).await
    }

    pub async fn get_conformance(&self) -> anyhow::Result<Value> {
        self.get("/conformance", &[]).await
    }

    pub async fn list_collections(&self) -> anyhow::Result<Value> {
        let data = self.get("/collections", &[]).await?;

        Ok(data
            .get("collections")
            .cloned()
            .unwrap_or_else(|| json!([])))
    }

    pub async fn get_collection(
        &self,
        collection_id: &str,
    ) -> anyhow::Result<Value> {
        let path = format!("/collections/{}", collection_id);

        self.get(&path, &[]).await
    }

    pub async fn list_instances(
        &self,
        collection_id: &str,
    ) -> anyhow::Result<Value> {
        let path = format!(
            "/collections/{}/instances",
            collection_id
        );

        let data = self.get(&path, &[]).await?;

        Ok(data
            .get("instances")
            .cloned()
            .unwrap_or_else(|| json!([])))
    }

    /// All instances of a collection, newest first (the API list is not
    /// guaranteed to be chronological; ISO-8601 ids sort lexicographically).
    pub async fn instance_candidates(
        &self,
        collection_id: &str,
    ) -> anyhow::Result<Vec<InstanceInfo>> {
        if let Some(v) = cache_get(&self.instances_cache, collection_id, INSTANCE_TTL) {
            return Ok(v);
        }
        let instances = self.list_instances(collection_id).await?;
        let mut list: Vec<InstanceInfo> = instances
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid instances response"))?
            .iter()
            .filter_map(|i| {
                let id = i.get("id")?.as_str()?.to_string();
                let interval = i.pointer("/extent/temporal/interval/0");
                let at = |k: usize| {
                    interval
                        .and_then(|iv| iv.get(k))
                        .and_then(Value::as_str)
                        .map(String::from)
                };
                Some(InstanceInfo { id, end: at(1) })
            })
            .collect();
        list.sort_by(|a, b| b.id.cmp(&a.id));
        list.dedup_by(|a, b| a.id == b.id);
        if list.is_empty() {
            anyhow::bail!("No instances available for collection {}", collection_id);
        }
        cache_put(&self.instances_cache, collection_id, list.clone());
        Ok(list)
    }

    /// Newest instance that actually serves data. Probes a single point/param
    /// of the newest runs and falls back to older ones if a run is not (yet)
    /// queryable. Cached for two minutes.
    pub async fn get_latest_instance_id(
        &self,
        collection_id: &str,
    ) -> anyhow::Result<String> {
        if let Some(v) = cache_get(&self.latest_cache, collection_id, INSTANCE_TTL) {
            return Ok(v);
        }
        let candidates = self.instance_candidates(collection_id).await?;
        let probe_param = match self.collection_param_names(collection_id).await {
            Ok(names) if names.iter().any(|n| n == "T_2M") => "T_2M".to_string(),
            Ok(names) if !names.is_empty() => names[0].clone(),
            _ => "T_2M".to_string(),
        };
        for inst in candidates.iter().take(4) {
            if self
                .instance_has_data(collection_id, &inst.id, &probe_param)
                .await
            {
                cache_put(&self.latest_cache, collection_id, inst.id.clone());
                return Ok(inst.id.clone());
            }
        }
        // Nothing verified: fall back to the newest id rather than failing, and cache it
        // so the probes are not repeated on every call. Callers retry an older run if
        // this one turns out not to serve data.
        cache_put(&self.latest_cache, collection_id, candidates[0].id.clone());
        Ok(candidates[0].id.clone())
    }

    async fn instance_has_data(&self, collection_id: &str, instance_id: &str, param: &str) -> bool {
        let res = self
            .get_position_raw(
                collection_id,
                "POINT(10.45 51.16)",
                instance_id,
                Some(param),
                None,
                None,
                "CoverageJSON",
            )
            .await;
        match res {
            Ok(v) => v
                .pointer("/coverages/0/ranges")
                .and_then(Value::as_object)
                .and_then(|r| r.get(param))
                .and_then(|r| r.get("values"))
                .and_then(Value::as_array)
                .is_some_and(|vals| vals.iter().any(|x| x.is_number())),
            Err(_) => false,
        }
    }

    /// Collection metadata (cached).
    pub async fn collection_cached(&self, collection_id: &str) -> anyhow::Result<Value> {
        if let Some(v) = cache_get(&self.collection_cache, collection_id, COLLECTION_TTL) {
            return Ok(v);
        }
        let v = self.get_collection(collection_id).await?;
        cache_put(&self.collection_cache, collection_id, v.clone());
        Ok(v)
    }

    /// Parameter names offered by a collection.
    pub async fn collection_param_names(&self, collection_id: &str) -> anyhow::Result<Vec<String>> {
        let v = self.collection_cached(collection_id).await?;
        Ok(v.get("parameter_names")
            .and_then(Value::as_object)
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default())
    }

    /// Position query for several points at once (WKT MULTIPOINT). Note: the
    /// API de-duplicates cells and does not preserve the point order.
    pub async fn get_multipoint_raw(
        &self,
        collection_id: &str,
        instance_id: &str,
        points: &[(f64, f64)],
        parameter_names: &str,
        datetime: Option<&str>,
    ) -> anyhow::Result<Value> {
        let coords = format!(
            "MULTIPOINT({})",
            points
                .iter()
                .map(|(x, y)| format!("({x:.4} {y:.4})"))
                .collect::<Vec<_>>()
                .join(",")
        );
        self.get_position_raw(
            collection_id,
            &coords,
            instance_id,
            Some(parameter_names),
            datetime,
            None,
            "CoverageJSON",
        )
        .await
    }

    pub async fn get_position_raw(
        &self,
        collection_id: &str,
        coords: &str,
        instance_id: &str,
        parameter_names: Option<&str>,
        datetime: Option<&str>,
        crs: Option<&str>,
        output_format: &str,
    ) -> anyhow::Result<Value> {
        let path = format!(
                "/collections/{}/instances/{}/position",
                collection_id, instance_id);

        let params = optional_params(vec![
            ("coords", Some(coords.to_string())),
            ("parameter-name", parameter_names.map(String::from)),
            ("datetime", datetime.map(String::from)),
            ("crs", crs.map(String::from)),
            ("f", Some(output_format.to_string())),
        ]);

        self.get(&path, &params).await
    }

    pub async fn get_radius_raw(
        &self,
        collection_id: &str,
        coords: &str,
        within: f64,
        within_units: &str,
        instance_id: Option<&str>,
        parameter_names: Option<&str>,
        datetime: Option<&str>,
        crs: Option<&str>,
        output_format: &str,
    ) -> anyhow::Result<Value> {
        let path = match instance_id {
            Some(instance) => format!(
                "/collections/{}/instances/{}/radius",
                collection_id, instance
            ),
            None => format!(
                "/collections/{}/radius",
                collection_id
            ),
        };

        let params = optional_params(vec![
            ("coords", Some(coords.to_string())),
            ("within", Some(within.to_string())),
            ("within-units", Some(within_units.to_string())),
            ("parameter-name", parameter_names.map(String::from)),
            ("datetime", datetime.map(String::from)),
            ("crs", crs.map(String::from)),
            ("f", Some(output_format.to_string())),
        ]);

        self.get(&path, &params).await
    }

    pub async fn get_area_raw(
        &self,
        collection_id: &str,
        coords: &str,
        instance_id: Option<&str>,
        parameter_names: Option<&str>,
        datetime: Option<&str>,
        crs: Option<&str>,
        output_format: &str,
    ) -> anyhow::Result<Value> {
        let path = match instance_id {
            Some(instance) => format!(
                "/collections/{}/instances/{}/area",
                collection_id, instance
            ),
            None => format!(
                "/collections/{}/area",
                collection_id
            ),
        };

        let params = optional_params(vec![
            ("coords", Some(coords.to_string())),
            ("parameter-name", parameter_names.map(String::from)),
            ("datetime", datetime.map(String::from)),
            ("crs", crs.map(String::from)),
            ("f", Some(output_format.to_string())),
        ]);

        self.get(&path, &params).await
    }

    pub async fn get_cube_raw(
        &self,
        collection_id: &str,
        bbox: &str,
        instance_id: Option<&str>,
        parameter_names: Option<&str>,
        datetime: Option<&str>,
        crs: Option<&str>,
        output_format: &str,
    ) -> anyhow::Result<Value> {
        let path = match instance_id {
            Some(instance) => format!(
                "/collections/{}/instances/{}/cube",
                collection_id, instance
            ),
            None => format!(
                "/collections/{}/cube",
                collection_id
            ),
        };

        let params = optional_params(vec![
            ("bbox", Some(bbox.to_string())),
            ("parameter-name", parameter_names.map(String::from)),
            ("datetime", datetime.map(String::from)),
            ("crs", crs.map(String::from)),
            ("f", Some(output_format.to_string())),
        ]);

        self.get(&path, &params).await
    }

    pub async fn get_point_forecast(
        &self,
        latitude: f64,
        longitude: f64,
        collection_id: &str,
        instance_id: Option<&str>,
        parameters: Option<Vec<String>>,
        datetime_range: Option<&str>,
    ) -> anyhow::Result<Value> {
        let default_params = vec![
            "T_2M",
            "TOT_PREC",
            "U_10M",
            "V_10M",
            "CLCT",
            "PMSL",
            "WW",
        ];

        let param_list: Vec<String> = match parameters {
            Some(params) => params,
            None => default_params
                .into_iter()
                .map(String::from)
                .collect(),
        };

        let param_str = param_list.join(",");

        let coords = format!(
            "POINT({} {})",
            longitude, latitude
        );

        let fetch = |instance: String| {
            let coords = coords.clone();
            let param_str = param_str.clone();
            async move {
                self.get_position_raw(
                    collection_id,
                    &coords,
                    &instance,
                    Some(&param_str),
                    datetime_range,
                    None,
                    "CoverageJSON",
                )
                .await
            }
        };

        let (instance_id, raw_data) = match instance_id {
            Some(id) => (id.to_string(), fetch(id.to_string()).await?),
            None => {
                let latest = self.get_latest_instance_id(collection_id).await?;
                match fetch(latest.clone()).await {
                    Ok(raw) => (latest, raw),
                    // The newest run may be listed before it is queryable: try the next older one.
                    Err(err) if !is_client_error(&err) => {
                        let older = self
                            .instance_candidates(collection_id)
                            .await
                            .ok()
                            .and_then(|c| c.into_iter().find(|i| i.id < latest));
                        match older {
                            Some(prev) => {
                                let raw = fetch(prev.id.clone()).await.map_err(|_| err)?;
                                (prev.id, raw)
                            }
                            None => return Err(err),
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
        };

        self.parse_point_forecast(
            raw_data,
            collection_id,
            &instance_id,
            latitude,
            longitude,
            &param_list,
        )
    }

    fn parse_point_forecast(
        &self,
        raw_data: Value,
        collection_id: &str,
        instance_id: &str,
        latitude: f64,
        longitude: f64,
        param_list: &[String],
    ) -> anyhow::Result<Value> {
        let coverages = raw_data
            .get("coverages")
            .and_then(Value::as_array);

        let Some(coverages) = coverages else {
            return Ok(json!({
                "collection_id": collection_id,
                "instance_id": instance_id,
                "latitude": latitude,
                "longitude": longitude,
                "timestamps": [],
                "forecast": [],
                "raw_response": raw_data
            }));
        };

        if coverages.is_empty() {
            return Ok(json!({
                "collection_id": collection_id,
                "instance_id": instance_id,
                "latitude": latitude,
                "longitude": longitude,
                "timestamps": [],
                "forecast": [],
                "raw_response": raw_data
            }));
        }

        let coverage = &coverages[0];

        let timestamps = coverage
            .get("domain")
            .and_then(|v| v.get("axes"))
            .and_then(|v| v.get("t"))
            .and_then(|v| v.get("values"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let ranges = coverage
            .get("ranges")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        let forecast_reference_time = coverage
            .get("cf:forecast_reference_time")
            .and_then(Value::as_str)
            .unwrap_or(instance_id);

        let mut forecast = Vec::new();

        for (i, timestamp) in timestamps.iter().enumerate() {
            let mut point = serde_json::Map::new();

            point.insert(
                "time".to_string(),
                timestamp.clone(),
            );

            for param in param_list {
                if let Some(range) = ranges.get(param) {
                    if let Some(values) =
                        range.get("values").and_then(Value::as_array)
                    {
                        if let Some(value) = values.get(i) {
                            if !value.is_null() {
                                if (param == "T_2M"
                                    || param == "TD_2M")
                                    && value.is_number()
                                {
                                    if let Some(kelvin) =
                                        value.as_f64()
                                    {
                                        point.insert(
                                            format!(
                                                "{}_celsius",
                                                param
                                            ),
                                            json!(
                                                ((kelvin - 273.15)
                                                    * 100.0)
                                                    .round()
                                                    / 100.0
                                            ),
                                        );
                                    }
                                }

                                point.insert(
                                    param.clone(),
                                    value.clone(),
                                );
                            }
                        }
                    }
                }
            }

            forecast.push(Value::Object(point));
        }

        Ok(json!({
            "collection_id": collection_id,
            "instance_id": instance_id,
            "forecast_reference_time": forecast_reference_time,
            "latitude": latitude,
            "longitude": longitude,
            "total_timesteps": timestamps.len(),
            "forecast": forecast
        }))
    }
}

fn optional_params(
    params: Vec<(&str, Option<String>)>,
) -> Vec<(&str, String)> {
    params
        .into_iter()
        .filter_map(|(key, value)| {
            value.map(|value| (key, value))
        })
        .collect()
}