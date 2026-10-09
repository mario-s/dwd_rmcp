use reqwest::Client;
use serde_json::{json, Value};

pub const DWD_BASE_URL: &str =
    "https://nwp.opendata-api.dwd.de/v1beta1";

// Default collection ID for the EDR API should be: "ICON-D2-RUC@single_level"

#[derive(Clone)]
pub struct DwdEdrClient {
    client: Client,
    base_url: String,
}

impl DwdEdrClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
            base_url: DWD_BASE_URL.trim_end_matches('/').to_string(),
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
            anyhow::bail!(
                "DWD API returned HTTP {}\nServer response: {}",
                status,
                body
            );
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
        instance_id: &str,
        parameter_names: Option<&str>,
        datetime: Option<&str>,
        crs: Option<&str>,
        output_format: &str,
    ) -> anyhow::Result<Value> {
        let path = format!(
                "/collections/{}/instances/{}/radius",
                collection_id, instance_id);

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
        instance_id: &str,
        parameter_names: Option<&str>,
        datetime: Option<&str>,
        crs: Option<&str>,
        output_format: &str,
    ) -> anyhow::Result<Value> {
        let path = format!(
                "/collections/{}/instances/{}/area",
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

    pub async fn get_cube_raw(
        &self,
        collection_id: &str,
        bbox: &str,
        instance_id: &str,
        parameter_names: Option<&str>,
        datetime: Option<&str>,
        crs: Option<&str>,
        output_format: &str,
    ) -> anyhow::Result<Value> {
        let path = format!(
                "/collections/{}/instances/{}/cube",
                collection_id, instance_id);

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
        instance_id: &str,
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

        let raw_data = self
            .get_position_raw(
                collection_id,
                &coords,
                &instance_id,
                Some(&param_str),
                datetime_range,
                None,
                "CoverageJSON",
            )
            .await?;

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