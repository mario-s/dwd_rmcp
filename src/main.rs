mod dwd_client;
mod shaping;
mod show_tools;
mod widgets;

use std::sync::Arc;
use dwd_client::{DwdEdrClient, DEFAULT_COLLECTION};
use rmcp::{
    ErrorData, RoleServer, ServiceExt,
    model::{
        ExtensionCapabilities, Implementation, ListResourcesResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
        ResourceContents, ServerCapabilities, ServerConfig,
    },
    schemars::JsonSchema,
    service::RequestContext,
    tool,
    tool_handler,
    tool_router,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    transport::stdio
};
use serde::Deserialize;

// -----------------------------------------------------------------------------
// Tool parameter types
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct CollectionParams {
    #[schemars(
        description = "Collection ID, e.g. ICON-D2-RUC@single_level"
    )]
    collection_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PointForecastParams {
    #[schemars(description = "Latitude in degrees")]
    latitude: f64,

    #[schemars(description = "Longitude in degrees")]
    longitude: f64,

    #[schemars(
        description = "Model collection name, e.g. ICON-D2-RUC@single_level"
    )]
    collection_id: Option<String>,

    #[schemars(
        description = "Specific model run instance ID. If omitted, the latest run is used."
    )]
    instance_id: Option<String>,

    #[schemars(
        description = "Specific parameter names, e.g. ['T_2M', 'TOT_PREC', 'U_10M', 'V_10M', 'CLCT']"
    )]
    parameters: Option<Vec<String>>,

    #[schemars(
        description = "Optional ISO8601 timestamp or interval"
    )]
    datetime_range: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PositionParams {
    #[schemars(
        description = "WKT Point, e.g. POINT(13.405 52.520)"
    )]
    coords: String,

    #[schemars(
        description = "Collection ID, e.g. ICON-D2-RUC@single_level"
    )]
    collection_id: Option<String>,

    #[schemars(
        description = "Specific model run instance ID"
    )]
    instance_id: Option<String>,

    #[schemars(
        description = "Comma-separated parameter names, e.g. T_2M,TOT_PREC"
    )]
    parameter_names: Option<String>,

    #[schemars(
        description = "ISO8601 timestamp or interval"
    )]
    datetime_val: Option<String>,

    #[schemars(
        description = "Coordinate reference system identifier"
    )]
    crs: Option<String>,

    #[schemars(
        description = "Output format, e.g. CoverageJSON"
    )]
    output_format: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RadiusParams {
    #[schemars(
        description = "Center point as WKT, e.g. POINT(13.405 52.520)"
    )]
    coords: String,

    #[schemars(
        description = "Radius distance"
    )]
    within: f64,

    #[schemars(
        description = "Radius units, e.g. km, m, mi"
    )]
    within_units: Option<String>,

    #[schemars(
        description = "Collection ID"
    )]
    collection_id: Option<String>,

    #[schemars(
        description = "Specific model run instance ID"
    )]
    instance_id: Option<String>,

    #[schemars(
        description = "Comma-separated parameter names"
    )]
    parameter_names: Option<String>,

    #[schemars(
        description = "ISO8601 timestamp or interval"
    )]
    datetime_val: Option<String>,

    #[schemars(
        description = "Coordinate reference system identifier"
    )]
    crs: Option<String>,

    #[schemars(
        description = "Output format, e.g. CoverageJSON"
    )]
    output_format: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AreaParams {
    #[schemars(
        description = "WKT Polygon, e.g. POLYGON((13.3 52.4, 13.5 52.4, 13.5 52.6, 13.3 52.6, 13.3 52.4))"
    )]
    coords: String,

    #[schemars(
        description = "Collection ID"
    )]
    collection_id: Option<String>,

    #[schemars(
        description = "Specific model run instance ID"
    )]
    instance_id: Option<String>,

    #[schemars(
        description = "Comma-separated parameter names"
    )]
    parameter_names: Option<String>,

    #[schemars(
        description = "ISO8601 timestamp or interval"
    )]
    datetime_val: Option<String>,

    #[schemars(
        description = "Coordinate reference system identifier"
    )]
    crs: Option<String>,

    #[schemars(
        description = "Output format, e.g. CoverageJSON"
    )]
    output_format: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CubeParams {
    #[schemars(
        description = "Bounding box minx,miny,maxx,maxy, e.g. 13.3,52.4,13.5,52.6"
    )]
    bbox: String,

    #[schemars(
        description = "Collection ID"
    )]
    collection_id: Option<String>,

    #[schemars(
        description = "Specific model run instance ID"
    )]
    instance_id: Option<String>,

    #[schemars(
        description = "Comma-separated parameter names"
    )]
    parameter_names: Option<String>,

    #[schemars(
        description = "ISO8601 timestamp or interval"
    )]
    datetime_val: Option<String>,

    #[schemars(
        description = "Coordinate reference system identifier"
    )]
    crs: Option<String>,

    #[schemars(
        description = "Output format, e.g. CoverageJSON"
    )]
    output_format: Option<String>,
}

// -----------------------------------------------------------------------------
// MCP Server
// -----------------------------------------------------------------------------

#[derive(Clone)]
struct DwdMcpServer {
    client: Arc<DwdEdrClient>,
    tool_router: ToolRouter<Self>,
}

const INSTRUCTIONS: &str = "DWD (Deutscher Wetterdienst) ICON-D2-RUC weather model data for Germany and neighbouring countries \
(hourly runs, about 27 h ahead, ~2.2 km). When the user wants to SEE weather, prefer the show_* tools, which render \
interactive widgets: show_current_weather (now), show_forecast_chart (hourly meteogram), show_wind_profile, \
show_thunderstorm_risk, show_location_compare (2-6 places), show_area_map (gridded map with time slider) and \
show_model_runs (data sources). They need latitude/longitude; geocode place names yourself. Use the get_*/query_* \
tools only for raw data questions. Widgets already display the numbers, so keep the follow-up text short.";

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for DwdMcpServer {
    fn get_info(&self) -> ServerConfig {
        let mut extensions = ExtensionCapabilities::new();
        if let Ok(ui) = serde_json::from_value(serde_json::json!({
            "mimeTypes": [widgets::MIME_TYPE]
        })) {
            extensions.insert("io.modelcontextprotocol/ui".to_string(), ui);
        }
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_extensions_with(extensions)
                .build(),
        )
        .with_server_info(
            Implementation::new("dwd-rmcp", env!("CARGO_PKG_VERSION"))
                .with_title("DWD Wetter (ICON-D2)"),
        )
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let resources = widgets::WIDGETS
            .iter()
            .map(|w| {
                Resource::new(w.uri(), w.name)
                    .with_title(w.title)
                    .with_description(w.description)
                    .with_mime_type(widgets::MIME_TYPE)
                    .with_meta(widgets::resource_meta())
            })
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let widget = widgets::find_by_uri(&request.uri).ok_or_else(|| {
            ErrorData::resource_not_found(
                format!("unknown resource {}", request.uri),
                None,
            )
        })?;
        let contents = ResourceContents::TextResourceContents {
            uri: widget.uri(),
            mime_type: Some(widgets::MIME_TYPE.to_string()),
            text: widget.render(),
            meta: Some(widgets::resource_meta()),
        };
        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

impl Default for DwdMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

// -----------------------------------------------------------------------------
// MCP tools
// -----------------------------------------------------------------------------
#[tool_router(router = tool_router)]
impl DwdMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router() + Self::show_router(),
            client: Arc::new(DwdEdrClient::new()),
        }
    }

    // -------------------------------------------------------------------------
    // get_dwd_api_info
    // -------------------------------------------------------------------------

    #[tool(
        name = "get_dwd_api_info",
        description = "Get general API information and landing page links from the DWD EDR API."
    )]
    async fn get_dwd_api_info(&self) -> String {
        match self.client.get_landing_page().await {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }

    #[tool(
        name = "get_conformance",
        description = "Get general API information and landing page links from the DWD EDR API."
    )]
    pub async fn get_conformance(&self) -> String {
        match self.client.get_conformance().await {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }

    // -------------------------------------------------------------------------
    // list_model_collections
    // -------------------------------------------------------------------------

    #[tool(
        name = "list_model_collections",
        description = "List all available weather model data collections in DWD EDR API, e.g. ICON-D2-RUC@single_level."
    )]
    pub async fn list_model_collections(&self) -> String {
        match self.client.list_collections().await {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }

    // -------------------------------------------------------------------------
    // describe_model_collection
    // -------------------------------------------------------------------------

    #[tool(
        name = "describe_model_collection",
        description = "Get metadata for a specific model collection, including available parameter names and query types."
    )]
    pub async fn describe_model_collection(
        &self,
        Parameters(params): Parameters<CollectionParams>,
    ) -> String {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        match self.client.get_collection(collection_id).await {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }

    // -------------------------------------------------------------------------
    // list_model_run_instances
    // -------------------------------------------------------------------------

    #[tool(
        name = "list_model_run_instances",
        description = "List all available model run instances (forecast reference runs) for a collection."
    )]
    pub async fn list_model_run_instances(
        &self,
        Parameters(params): Parameters<CollectionParams>,
    ) -> String {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        match self.client.list_instances(collection_id).await {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }

    // -------------------------------------------------------------------------
    // get_point_weather_forecast
    // -------------------------------------------------------------------------

    #[tool(
        name = "get_point_weather_forecast",
        description = "Get a high-level parsed weather forecast time series for a specific location (latitude/longitude). Returns temperature, precipitation, wind components, cloud cover, pressure and weather code."
    )]
    pub async fn get_point_weather_forecast(
        &self,
        Parameters(params): Parameters<PointForecastParams>,
    ) -> String {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        match self
            .client
            .get_point_forecast(
                params.latitude,
                params.longitude,
                collection_id,
                params.instance_id.as_deref(),
                params.parameters,
                params.datetime_range.as_deref(),
            )
            .await
        {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }

    // -------------------------------------------------------------------------
    // query_edr_position
    // -------------------------------------------------------------------------

    #[tool(
        name = "query_edr_position",
        description = "Query the DWD EDR API position endpoint using WKT POINT coordinates and return raw CoverageJSON data."
    )]
    pub async fn query_edr_position(
        &self,
        Parameters(params): Parameters<PositionParams>,
    ) -> String {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let output_format = params
            .output_format
            .as_deref()
            .unwrap_or("CoverageJSON");

        match self
            .client
            .get_position_raw(
                collection_id,
                &params.coords,
                params.instance_id.as_deref(),
                params.parameter_names.as_deref(),
                params.datetime_val.as_deref(),
                params.crs.as_deref(),
                output_format,
            )
            .await
        {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }

    // -------------------------------------------------------------------------
    // query_edr_radius
    // -------------------------------------------------------------------------

    #[tool(
        name = "query_edr_radius",
        description = "Query the DWD EDR API radius endpoint and return coverage data around a center coordinate."
    )]
    pub async fn query_edr_radius(
        &self,
        Parameters(params): Parameters<RadiusParams>,
    ) -> String {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let within_units = params
            .within_units
            .as_deref()
            .unwrap_or("km");

        let output_format = params
            .output_format
            .as_deref()
            .unwrap_or("CoverageJSON");

        match self
            .client
            .get_radius_raw(
                collection_id,
                &params.coords,
                params.within,
                within_units,
                params.instance_id.as_deref(),
                params.parameter_names.as_deref(),
                params.datetime_val.as_deref(),
                params.crs.as_deref(),
                output_format,
            )
            .await
        {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }

    // -------------------------------------------------------------------------
    // query_edr_area
    // -------------------------------------------------------------------------

    #[tool(
        name = "query_edr_area",
        description = "Query the DWD EDR API area endpoint using WKT POLYGON coordinates."
    )]
    pub async fn query_edr_area(
        &self,
        Parameters(params): Parameters<AreaParams>,
    ) -> String {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let output_format = params
            .output_format
            .as_deref()
            .unwrap_or("CoverageJSON");

        match self
            .client
            .get_area_raw(
                collection_id,
                &params.coords,
                params.instance_id.as_deref(),
                params.parameter_names.as_deref(),
                params.datetime_val.as_deref(),
                params.crs.as_deref(),
                output_format,
            )
            .await
        {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }

    // -------------------------------------------------------------------------
    // query_edr_cube
    // -------------------------------------------------------------------------

    #[tool(
        name = "query_edr_cube",
        description = "Query the DWD EDR API cube endpoint for a bounding box volume."
    )]
    pub async fn query_edr_cube(
        &self,
        Parameters(params): Parameters<CubeParams>,
    ) -> String {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let output_format = params
            .output_format
            .as_deref()
            .unwrap_or("CoverageJSON");

        match self
            .client
            .get_cube_raw(
                collection_id,
                &params.bbox,
                params.instance_id.as_deref(),
                params.parameter_names.as_deref(),
                params.datetime_val.as_deref(),
                params.crs.as_deref(),
                output_format,
            )
            .await
        {
            Ok(data) => pretty_json(data),
            Err(err) => format!("DWD API error: {}", err),
        }
    }
}

// -----------------------------------------------------------------------------
// Helper
// -----------------------------------------------------------------------------

fn pretty_json(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value)
        .unwrap_or_else(|err| {
            format!("JSON serialization error: {}", err)
        })
}

// -----------------------------------------------------------------------------
// Main
// -----------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = DwdMcpServer::new();

    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}