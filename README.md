# dwd_rmcp
Rust based MCP server for DWD.
It uses the [Environmental Data Retrieval (EDR) API](https://nwp.opendata-api.dwd.de/v1beta1/docs) by Deutscher Wetterdienst (DWD).

### Build
`cargo buil --release`

### Include
To use it in an agent just add the path to the binary. It uses stdio, no further arguments are required.
```
"mcpServers": {
    "rust-weather-server": {
      "command": "/Users/donald.duck/rust/dwd_rmcp/target/release/dwd-mcp-server"
    },
}
```