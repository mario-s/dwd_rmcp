mod client;
mod server;

use rmcp::{
    ServiceExt,
    transport::stdio
};
use crate::server::dwd_mcp::DwdMcpServer;
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