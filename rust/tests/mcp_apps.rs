//! Exercise composition at the rmcp boundary, without pretending to render an iframe.
#[path = "support/app_contract.rs"]
mod contract;
#[path = "../examples/mcp_app.rs"]
mod example;

use mcp_apps_server::{AppRouter, AppServer, StaticUiResource, UiResource, UiResourceUri};
use openai_mcp_extensions::{
    Error, OpenAIRequestContextExt, OpenAIUiToolExt,
    ui::{DisplayMode, UiToolMetadata},
};
use rmcp::{ErrorData, RoleServer, ServerHandler, service::RequestContext, tool};
use serde_json::json;

struct InlineView;

impl StaticUiResource for InlineView {
    fn resource() -> Result<UiResource, mcp_apps_server::UiUriError> {
        Ok(UiResource::new(
            UiResourceUri::new("ui://parts/viewer")?,
            "parts-viewer",
        ))
    }

    fn html() -> &'static str {
        include_str!("../examples/support/parts.html")
    }

    fn ui() -> mcp_apps_server::ResourceUi {
        example_ui()
    }

    fn meta() -> Result<rmcp::model::MetaObject, mcp_apps_server::MetadataError> {
        example_meta()
    }
}

fn example_ui() -> mcp_apps_server::ResourceUi {
    let mut ui = mcp_apps_server::ResourceUi::default();
    ui.prefers_border = Some(false);
    ui
}

fn example_meta() -> Result<rmcp::model::MetaObject, mcp_apps_server::MetadataError> {
    let mut meta = rmcp::model::MetaObject::default();
    openai_mcp_extensions::ui::UiResourceMetadata {
        available_display_modes: Some(vec![DisplayMode::Inline, DisplayMode::Fullscreen]),
        preferred_display_mode: Some(DisplayMode::Inline),
    }
    .apply(&mut meta)
    .map_err(|error| mcp_apps_server::MetadataError::Preparation(Box::new(error)))?;
    Ok(meta)
}

struct InlineCatalog;
impl InlineCatalog {
    #[tool(description = "Read a catalog with an inline interface")]
    fn open_catalog(&self, context: RequestContext<RoleServer>) -> Result<String, ErrorData> {
        // Make forwarding of the original request metadata observable through
        // the OpenAI SDK reader rather than directly inspecting JSON fields.
        let client = context.openai_client().map_err(ErrorData::from)?;
        let session = client
            .session_id()
            .ok_or_else(|| ErrorData::invalid_params("missing session", None))?;
        Ok(format!("Catalog for {session}"))
    }
}

#[tokio::test]
async fn ready_example_serves_openai_entrypoints_and_generic_resource_metadata() {
    let observed = contract::observe(example::catalog_server().unwrap()).await;
    contract::assert_catalog_contract(&observed);
}

#[tokio::test]
async fn optional_app_router_preserves_openai_annotations_and_request_context() {
    let mut apps = AppRouter::new();
    apps.register_static::<InlineView>().unwrap();
    apps.register_tool((
        InlineCatalog::open_catalog_tool_attr(),
        InlineCatalog::open_catalog,
    ))
    .unwrap();
    apps.associate_static_tool::<InlineView>("open_catalog")
        .unwrap();
    let server = AppServer::new(InlineCatalog, apps).unwrap();
    let observed = contract::observe(server).await;
    assert_eq!(
        observed["tools"][0]["_meta"]["ui"]["resourceUri"],
        "ui://parts/viewer"
    );
    assert_eq!(
        observed["contents"]["contents"][0]["_meta"],
        json!({
            "ui":{"prefersBorder":false},
            "openai/ui":{"availableDisplayModes":["inline","fullscreen"],"preferredDisplayMode":"inline"}
        })
    );
    assert_eq!(
        observed["result"]["content"][0]["text"],
        "Catalog for synthetic-app-session"
    );
}

#[test]
fn native_tool_association_conflicts_preserve_openai_annotations_and_error_causes() {
    let server = example::catalog_server().unwrap();
    let mut tool = server.get_tool("open_catalog").unwrap();
    let before = tool.clone();
    let error = tool
        .associate_app_resource("ui://parts/another-view")
        .unwrap_err();
    assert_eq!(tool, before);
    let source = std::error::Error::source(&error).unwrap();
    assert!(
        std::error::Error::source(source)
            .unwrap()
            .is::<mcp_apps_server::MetadataError>()
    );
    assert!(matches!(error, Error::Configuration { .. }));
    tool.apply_openai_ui(&UiToolMetadata::default()).unwrap();
    assert_eq!(tool.meta.unwrap().0["ui"], before.meta.unwrap().0["ui"]);
}
