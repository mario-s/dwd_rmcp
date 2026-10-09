mod dwd_client;
mod input_params;

use std::sync::Arc;
use dwd_client::{DwdEdrClient, DEFAULT_COLLECTION};
use input_params::*;
use rmcp::{
    ServiceExt,
    tool,
    tool_handler,
    tool_router,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    transport::stdio
};
use serde_json::Value;


// -----------------------------------------------------------------------------
// MCP Server
// -----------------------------------------------------------------------------

#[derive(Clone)]
struct DwdMcpServer {
    client: Arc<DwdEdrClient>,
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for DwdMcpServer {}

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
            tool_router: Self::tool_router(),
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
    async fn get_dwd_api_info(&self) -> Result<String, String> {
        let response = self.client.get_landing_page();
        as_json(response.await)
    }

    #[tool(
        name = "get_conformance",
        description = "Get general API information and landing page links from the DWD EDR API."
    )]
    pub async fn get_conformance(&self) -> Result<String, String> {
        let response = self.client.get_conformance();
        as_json(response.await)
    }

    // -------------------------------------------------------------------------
    // list_model_collections
    // -------------------------------------------------------------------------

    #[tool(
        name = "list_model_collections",
        description = "List all available weather model data collections in DWD EDR API, e.g. ICON-D2-RUC@single_level."
    )]
    pub async fn list_model_collections(&self) -> Result<String, String> {
        let response = self.client.list_collections();
        as_json(response.await)
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
    ) -> Result<String, String> {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let response = self.client.get_collection(collection_id);
        as_json(response.await)
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
    ) -> Result<String, String> {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let response = self.client.list_instances(collection_id);
        as_json(response.await)
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
    ) -> Result<String, String> {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let response = self
            .client
            .get_point_forecast(
                params.latitude,
                params.longitude,
                collection_id,
                params.instance_id.as_deref(),
                params.parameters,
                params.datetime_range.as_deref(),
            );
        as_json(response.await)
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
    ) -> Result<String, String> {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let output_format = params
            .output_format
            .as_deref()
            .unwrap_or("CoverageJSON");

        let response = self
            .client
            .get_position_raw(
                collection_id,
                &params.coords,
                params.instance_id.as_deref(),
                params.parameter_names.as_deref(),
                params.datetime_val.as_deref(),
                params.crs.as_deref(),
                output_format,
            );
        as_json(response.await)
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
    ) -> Result<String, String> {
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

        let response = self
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
            );
        as_json(response.await)
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
    ) -> Result<String, String> {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let output_format = params
            .output_format
            .as_deref()
            .unwrap_or("CoverageJSON");

        let response = self
            .client
            .get_area_raw(
                collection_id,
                &params.coords,
                params.instance_id.as_deref(),
                params.parameter_names.as_deref(),
                params.datetime_val.as_deref(),
                params.crs.as_deref(),
                output_format,
            );
        as_json(response.await)
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
    ) -> Result<String, String> {
        let collection_id = params
            .collection_id
            .as_deref()
            .unwrap_or(DEFAULT_COLLECTION);

        let output_format = params
            .output_format
            .as_deref()
            .unwrap_or("CoverageJSON");

        let response = self
            .client
            .get_cube_raw(
                collection_id,
                &params.bbox,
                params.instance_id.as_deref(),
                params.parameter_names.as_deref(),
                params.datetime_val.as_deref(),
                params.crs.as_deref(),
                output_format,
            );
        as_json(response.await)
    }
}

// -----------------------------------------------------------------------------
// Helper
// -----------------------------------------------------------------------------

fn as_json(result: Result<Value, anyhow::Error>) -> Result<String, String> {
    let data = result.map_err(|err| format!("DWD API error: {err}"))?;

    serde_json::to_string(&data)
            .map_err(|err| format!("JSON serialization error: {err}"))
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