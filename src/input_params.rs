use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// -----------------------------------------------------------------------------
// Tool parameter types
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CollectionParams {
    #[schemars(
        description = "Collection ID, e.g. ICON-D2-RUC@single_level"
    )]
    pub collection_id: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PointForecastParams {
    #[schemars(description = "Latitude in degrees")]
    pub latitude: f64,

    #[schemars(description = "Longitude in degrees")]
    pub longitude: f64,

    #[schemars(
        description = "Model collection name, e.g. ICON-D2-RUC@single_level"
    )]
    pub collection_id: String,

    #[schemars(
        description = "Specific model run instance ID. If omitted, the latest run is used."
    )]
    pub instance_id: String,

    #[schemars(
        description = "Specific parameter names, e.g. ['T_2M', 'TOT_PREC', 'U_10M', 'V_10M', 'CLCT']"
    )]
    pub parameters: Option<Vec<String>>,

    #[schemars(
        description = "Optional ISO8601 timestamp or interval"
    )]
    pub datetime_range: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PositionParams {
    #[schemars(
        description = "WKT Point, e.g. POINT(13.405 52.520)"
    )]
    pub coords: String,

    #[schemars(
        description = "Collection ID, e.g. ICON-D2-RUC@single_level"
    )]
    pub collection_id: String,

    #[schemars(
        description = "Specific model run instance ID"
    )]
    pub instance_id: String,

    #[schemars(
        description = "Comma-separated parameter names, e.g. T_2M,TOT_PREC"
    )]
    pub parameter_names: Option<String>,

    #[schemars(
        description = "ISO8601 timestamp or interval"
    )]
    pub datetime_val: Option<String>,

    #[schemars(
        description = "Coordinate reference system identifier"
    )]
    pub crs: Option<String>,

    #[schemars(
        description = "Output format, e.g. CoverageJSON"
    )]
    pub output_format: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RadiusParams {
    #[schemars(
        description = "Center point as WKT, e.g. POINT(13.405 52.520)"
    )]
    pub coords: String,

    #[schemars(
        description = "Radius distance"
    )]
    pub within: f64,

    #[schemars(
        description = "Radius units, e.g. km, m, mi"
    )]
    pub within_units: Option<String>,

    #[schemars(
        description = "Collection ID"
    )]
    pub collection_id: String,

    #[schemars(
        description = "Specific model run instance ID"
    )]
    pub instance_id: String,

    #[schemars(
        description = "Comma-separated parameter names"
    )]
    pub parameter_names: Option<String>,

    #[schemars(
        description = "ISO8601 timestamp or interval"
    )]
    pub datetime_val: Option<String>,

    #[schemars(
        description = "Coordinate reference system identifier"
    )]
    pub crs: Option<String>,

    #[schemars(
        description = "Output format, e.g. CoverageJSON"
    )]
    pub output_format: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct AreaParams {
    #[schemars(
        description = "WKT Polygon, e.g. POLYGON((13.3 52.4, 13.5 52.4, 13.5 52.6, 13.3 52.6, 13.3 52.4))"
    )]
    pub coords: String,

    #[schemars(
        description = "Collection ID"
    )]
    pub collection_id: String,

    #[schemars(
        description = "Specific model run instance ID"
    )]
    pub instance_id: String,

    #[schemars(
        description = "Comma-separated parameter names"
    )]
    pub parameter_names: Option<String>,

    #[schemars(
        description = "ISO8601 timestamp or interval"
    )]
    pub datetime_val: Option<String>,

    #[schemars(
        description = "Coordinate reference system identifier"
    )]
    pub crs: Option<String>,

    #[schemars(
        description = "Output format, e.g. CoverageJSON"
    )]
    pub output_format: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CubeParams {
    #[schemars(
        description = "Bounding box minx,miny,maxx,maxy, e.g. 13.3,52.4,13.5,52.6"
    )]
    pub bbox: String,

    #[schemars(
        description = "Collection ID"
    )]
    pub collection_id: String,

    #[schemars(
        description = "Specific model run instance ID"
    )]
    pub instance_id: String,

    #[schemars(
        description = "Comma-separated parameter names"
    )]
    pub parameter_names: Option<String>,

    #[schemars(
        description = "ISO8601 timestamp or interval"
    )]
    pub datetime_val: Option<String>,

    #[schemars(
        description = "Coordinate reference system identifier"
    )]
    pub crs: Option<String>,

    #[schemars(
        description = "Output format, e.g. CoverageJSON"
    )]
    pub output_format: Option<String>,
}
