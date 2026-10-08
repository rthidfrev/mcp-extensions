//! The same tool definition and result used by both MCP App integration examples.
use openai_mcp_extensions::{
    Error, OpenAIUiToolExt,
    ui::{Entrypoint, UiToolMetadata},
};
use rmcp::{handler::server::wrapper::Json, model::Tool, schemars::JsonSchema, tool};
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct CatalogSnapshot {
    parts: Vec<String>,
}

pub(crate) struct Catalog;

impl Catalog {
    #[tool(
        description = "Open the parts catalog",
        annotations(read_only_hint = true)
    )]
    pub(crate) fn open_catalog(&self) -> Json<CatalogSnapshot> {
        Json(CatalogSnapshot {
            parts: vec!["bolt".into(), "washer".into(), "nut".into()],
        })
    }
}

pub(crate) fn tool_definition() -> Result<Tool, Error> {
    let mut definition = Catalog::open_catalog_tool_attr();
    definition.apply_openai_ui(&UiToolMetadata {
        entrypoints: Some(vec![
            Entrypoint::Global { quick_action: None },
            Entrypoint::Thread,
        ]),
        ..Default::default()
    })?;
    Ok(definition)
}
