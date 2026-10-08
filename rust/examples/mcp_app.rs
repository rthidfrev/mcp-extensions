//! A ready MCP Apps server with OpenAI global and thread entrypoints.
//! Run `cargo run --example mcp_app --features mcp-apps`; the browser resource remains static.
#[path = "support/app.rs"]
mod app;
#[path = "support/catalog.rs"]
mod catalog;

use catalog::Catalog;
use mcp_apps_server::{AppRouter, AppServer};
use rmcp::{ServiceExt, model::Implementation};

/// Assemble declarations; the caller still chooses the runtime and transport.
pub(crate) fn catalog_server() -> Result<AppServer<Catalog>, Box<dyn std::error::Error>> {
    let mut apps = AppRouter::new();
    apps.register_static::<app::PartsView>()?;
    apps.register_tool((catalog::tool_definition()?, Catalog::open_catalog))?;
    apps.associate_static_tool::<app::PartsView>("open_catalog")?;
    Ok(
        AppServer::new(Catalog, apps)?.with_server_info(Implementation::new(
            "openai-mcp-app-example",
            env!("CARGO_PKG_VERSION"),
        )),
    )
}

// Tests import this example without launching its stdio entry point.
#[allow(dead_code)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    catalog_server()?
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
