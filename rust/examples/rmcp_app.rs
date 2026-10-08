//! An MCP App served directly with rmcp and the SDK's core OpenAI annotations.
//! Run `cargo run --example rmcp_app --no-default-features`; no MCP Apps helper is compiled.
#[path = "support/catalog.rs"]
mod catalog;

use catalog::Catalog;
use openai_mcp_extensions::{
    Error, OpenAIUiResourceExt,
    ui::{DisplayMode, UiResourceMetadata},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
    },
    model::{
        CallToolRequestParams, CallToolResponse, ListResourcesResult, ListToolsResult, MetaObject,
        PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde::Serialize;

const APP_URI: &str = "ui://parts/viewer";
const APP_MIME_TYPE: &str = "text/html;profile=mcp-app";
const APP_HTML: &str = include_str!("support/parts.html");

// These fixed declarations belong to this example. A general registry must
// validate arbitrary identities, conflicting associations and resource ownership.
#[derive(Serialize)]
struct ToolMetadata {
    ui: ToolUi,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolUi {
    resource_uri: &'static str,
}

#[derive(Serialize)]
struct ResourceMetadata {
    ui: ResourceUi,
}

#[derive(Serialize)]
struct ResourceUi {
    csp: Csp,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Csp {
    connect_domains: Vec<String>,
    resource_domains: Vec<String>,
}

fn metadata(value: impl Serialize) -> Result<MetaObject, serde_json::Error> {
    serde_json::from_value(serde_json::to_value(value)?)
}

pub(crate) struct CatalogServer {
    catalog: Catalog,
    tools: ToolRouter<Catalog>,
    contents: ResourceContents,
}

impl CatalogServer {
    pub(crate) fn new() -> Result<Self, Error> {
        let mut definition = catalog::tool_definition()?;
        let association = metadata(ToolMetadata {
            ui: ToolUi {
                resource_uri: APP_URI,
            },
        })?;
        // The definition has only OpenAI metadata and the new association has
        // only `ui`; their namespaces are disjoint at this construction point.
        definition
            .meta
            .get_or_insert_default()
            .0
            .extend(association.0);
        let mut tools = ToolRouter::new();
        tools.add_route(ToolRoute::new(definition, Catalog::open_catalog));

        let security = metadata(ResourceMetadata {
            ui: ResourceUi {
                csp: Csp {
                    connect_domains: vec![],
                    resource_domains: vec![],
                },
            },
        })?;
        let mut contents = ResourceContents::text(APP_HTML, APP_URI)
            .with_mime_type(APP_MIME_TYPE)
            .with_meta(security);
        contents.apply_openai_ui(&UiResourceMetadata {
            available_display_modes: Some(vec![DisplayMode::Inline, DisplayMode::Fullscreen]),
            preferred_display_mode: Some(DisplayMode::Inline),
        })?;
        Ok(Self {
            catalog: Catalog,
            tools,
            contents,
        })
    }
}

impl ServerHandler for CatalogServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.server_info.name = "openai-rmcp-app-example".into();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info.capabilities
            .extensions
            .get_or_insert_default()
            .insert("io.modelcontextprotocol/ui".into(), Default::default());
        info
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.get(name).cloned()
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.list_all()))
    }

    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.tools
            .call(ToolCallContext::new(&self.catalog, params, context))
            .await
    }

    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(APP_URI, "parts-viewer").with_mime_type(APP_MIME_TYPE),
        ]))
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if params.uri != APP_URI {
            return Err(ErrorData::resource_not_found("Unknown resource URI", None));
        }
        Ok(ReadResourceResult::new(vec![self.contents.clone()]).into())
    }
}

// Tests import the example without launching its stdio entry point.
#[allow(dead_code)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    CatalogServer::new()?
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
