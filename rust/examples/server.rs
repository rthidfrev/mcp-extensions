//! A stdio server demonstrating MCP App declarations, settings, mentions and both form flows.
//! Run `cargo run --example server --features mcp-apps`; the host drives the interaction.
#[path = "support/app.rs"]
mod app;

use app::{APP_URI, PartsView};
use mcp_apps_server::AppRouter;
use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use openai_mcp_extensions::{
    OpenAIExtensions,
    forms::{
        self, OpenAIForm,
        mrtr::{FormInput, FormInputRequired, FormService, FormToolResponse, read_input},
    },
    mentions::{MentionItem, MentionSearchResult},
    resources::{FileEntrypointInput, ResourceRepresentation, requested_representation},
    settings::{SettingsGroup, SettingsLayoutItem, SettingsRegistration},
    ui::{Entrypoint, UiToolMetadata, associate_app_resource},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::router::tool::{ToolRoute, ToolRouter},
    model::{
        CallToolRequestParams, CallToolResponse, ListResourcesResult, ListToolsResult, MetaObject,
        ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
        Resource, ResourceContents, ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde_json::json;
use tokio::sync::Mutex;

const DATA_URI: &str = "parts://catalog/summary";
const SUMMARY: &str = "Hex bolt, washer and nut";

#[cfg(test)]
use app::APP_HTML;
#[cfg(test)]
use mcp_apps_server::UI_MIME_TYPE as APP_MIME_TYPE;

struct Server {
    router: AppRouter<Self>,
    extensions: OpenAIExtensions,
    choice: Arc<PartChoice>,
}

struct PendingChoice {
    arguments: serde_json::Map<String, serde_json::Value>,
    expires: Instant,
}

// Public, read-only catalog demonstration. These sequence handles are neither
// authentication nor secure tokens. Private/mutating applications must supply
// their own caller-bound, integrity-protected state and atomic effect storage.
struct PartChoice {
    form: Arc<OpenAIForm>,
    pending: Mutex<BTreeMap<String, PendingChoice>>,
    next: AtomicU64,
}

impl PartChoice {
    async fn handle(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<FormToolResponse, ErrorData> {
        let arguments = params.arguments.clone().unwrap_or_default();
        if !arguments.is_empty() {
            return Err(ErrorData::invalid_params(
                "choose_part accepts an empty object",
                None,
            ));
        }
        if context.ct.is_cancelled() {
            return Err(ErrorData::invalid_request("request cancelled", None));
        }
        match read_input(&params, "answer", &self.form).map_err(ErrorData::from)? {
            FormInput::Initial => {
                let now = Instant::now();
                let mut pending = self.pending.lock().await;
                pending.retain(|_, value| value.expires > now);
                if pending.len() >= 64 {
                    return Err(ErrorData::invalid_request(
                        "too many pending catalog choices",
                        None,
                    ));
                }
                let state = format!("part-choice-{}", self.next.fetch_add(1, Ordering::Relaxed));
                pending.insert(
                    state.clone(),
                    PendingChoice {
                        arguments,
                        expires: now + Duration::from_secs(600),
                    },
                );
                Ok(
                    FormInputRequired::new(state, "answer", "Choose a part", &self.form)
                        .map_err(ErrorData::from)?
                        .into(),
                )
            }
            FormInput::Response {
                request_state,
                result,
            } => {
                let mut pending = self.pending.lock().await;
                let original = pending.get(&request_state).ok_or_else(|| {
                    ErrorData::invalid_params("unknown or consumed choice state", None)
                })?;
                if original.arguments != arguments {
                    return Err(ErrorData::invalid_params("choice arguments changed", None));
                }
                if original.expires <= Instant::now() {
                    pending.remove(&request_state);
                    return Err(ErrorData::invalid_params("choice state expired", None));
                }
                pending.remove(&request_state);
                // Accept, decline and cancel all terminate this read-only
                // choice. Replay cannot reuse the consumed handle.
                Ok(rmcp::model::CallToolResult::structured(
                    serde_json::to_value(result)
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?,
                )
                .into())
            }
        }
    }
}

impl Server {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut router = ToolRouter::new();
        let mut extensions = OpenAIExtensions::new();
        let resource: Resource = serde_json::from_value(json!({
            "uri":"file:///parts/bolt.stl", "name":"bolt.stl", "title":"Hex bolt",
            "mimeType":"model/stl"
        }))?;
        extensions.register_mentions(&mut router, move |params, _context| {
            let resource = resource.clone();
            async move {
                let items = if "hex bolt".contains(&params.query.to_lowercase()) {
                    vec![MentionItem::ResourceLink(resource)]
                } else {
                    vec![]
                };
                Ok(MentionSearchResult { items })
            }
        })?;

        // This example stores values in memory for one process. Production
        // handlers must authorize the caller and persist per-user values.
        let values = Arc::new(Mutex::new(
            json!({"units":"mm","showGrid":true})
                .as_object()
                .unwrap()
                .clone(),
        ));
        let read_values = values.clone();
        extensions.register_settings(
            &mut router,
            SettingsRegistration::new(json!({
                "type":"object", "properties":{
                    "units":{"type":"string","title":"Units","enum":["mm","in"]},
                    "showGrid":{"type":"boolean","title":"Show grid"}
                }, "required":["units","showGrid"]
            }))?
            .with_layout(vec![SettingsGroup::Group {
                title: "Display".into(),
                items: vec![
                    SettingsLayoutItem::Property {
                        property: "units".into(),
                    },
                    SettingsLayoutItem::Property {
                        property: "showGrid".into(),
                    },
                    SettingsLayoutItem::Tool {
                        tool: "view_parts".into(),
                        title: "Browse parts".into(),
                        description: None,
                    },
                ],
            }])?,
            move |_context| {
                let values = read_values.clone();
                async move { Ok(values.lock().await.clone()) }
            },
            move |set, _context| {
                let values = values.clone();
                async move {
                    let mut values = values.lock().await;
                    values.extend(set);
                    Ok(values.clone())
                }
            },
        )?;

        let mut view = Tool::new("view_parts", "Open the parts tray", json!({
            "type":"object", "properties":{
                "file":{"type":"object","properties":{"name":{"type":"string"},"resourceUri":{"type":"string"}},"required":["name","resourceUri"]}
            }
        }).as_object().unwrap().clone());
        view.title = Some("Parts Tray".into());
        let mut meta = MetaObject::default();
        associate_app_resource(&mut meta, APP_URI)?;
        UiToolMetadata {
            entrypoints: Some(vec![
                Entrypoint::Global { quick_action: None },
                Entrypoint::Thread,
                Entrypoint::File {
                    extensions: vec![".stl".into()],
                },
            ]),
            ..Default::default()
        }
        .apply(&mut meta)?;
        view.meta = Some(meta);
        router.add_route(ToolRoute::new_dyn(view, |context| {
            Box::pin(async move {
                let arguments = context.arguments.unwrap_or_default();
                // Global/thread launches and settings buttons call this with {}.
                // File launches include an opaque resource URI, never a server path.
                let file = if arguments.contains_key("file") {
                    let input: FileEntrypointInput = serde_json::from_value(json!(arguments))
                        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
                    input.validate().map_err(ErrorData::from)?;
                    Some(input.file)
                } else {
                    None
                };
                Ok(rmcp::model::CallToolResult::structured(
                    json!({"parts":["bolt","washer","nut"],"file":file}),
                )
                .into())
            })
        }));

        let form = Arc::new(OpenAIForm::from_json(json!({
            "type":"object", "properties":{
                "part":{"type":"string","title":"Part","oneOf":[{
                    "const":"bolt", "title":"Hex bolt", "description":"A reusable CAD part",
                    "x-openai-thumbnail":{"src":"https://example.com/bolt.png"}
                }]}
            }, "required":["part"]
        }))?);
        let choice = Arc::new(PartChoice {
            form: form.clone(),
            pending: Default::default(),
            next: AtomicU64::new(0),
        });
        router.add_route(ToolRoute::new_dyn(
            Tool::new(
                "choose_part",
                "Select a CAD part using an OpenAI form",
                json!({"type":"object","properties":{}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            move |context| {
                let form = form.clone();
                Box::pin(async move {
                    let result =
                        forms::elicit_input(&context.request_context, "Choose a part", &form)
                            .await
                            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                    Ok(rmcp::model::CallToolResult::structured(
                        serde_json::to_value(result)
                            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?,
                    )
                    .into())
                })
            },
        ));
        let mut apps = AppRouter::from_tool_router(router);
        apps.register_static::<PartsView>()?;
        apps.validate()?;
        Ok(Self {
            router: apps,
            extensions,
            choice,
        })
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.server_info.name = "openai-extensions-example".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        self.extensions.apply_capabilities(&mut info.capabilities);
        self.router.declare_capabilities(&mut info.capabilities);
        info
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.router.get_tool(name)
    }

    // FormService handles modern choose_part calls; this handler keeps the
    // legacy route and all ordinary server features.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![
            ProtocolVersion::V_2026_07_28,
            ProtocolVersion::V_2025_11_25,
        ])
    }

    async fn list_tools(
        &self,
        request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.router.list_tools(request)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.router.call_tool(self, request, context).await
    }

    async fn list_resources(
        &self,
        request: Option<rmcp::model::PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let mut result = self.router.list_resources(request)?;
        result
            .resources
            .push(Resource::new(DATA_URI, "parts-summary").with_mime_type("text/plain"));
        Ok(result)
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        // This handler retains resource selection and can authorize before dispatch.
        if self.router.contains_resource(&request.uri) {
            return self.router.read_resource(self, request, context).await;
        }
        let contents = match request.uri.as_str() {
            DATA_URI => match requested_representation(&context.meta).map_err(ErrorData::from)? {
                Some(ResourceRepresentation::Blob) => {
                    use base64::Engine;
                    ResourceContents::blob(
                        base64::engine::general_purpose::STANDARD.encode(SUMMARY),
                        DATA_URI,
                    )
                    .with_mime_type("text/plain")
                }
                _ => ResourceContents::text(SUMMARY, DATA_URI),
            },
            _ => return Err(ErrorData::invalid_params("Unknown resource URI", None)),
        };
        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    service(Server::new()?)?
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

fn service(server: Server) -> Result<FormService<Server>, Box<dyn std::error::Error>> {
    let choice = server.choice.clone();
    let mut service = FormService::new(server);
    service.register("choose_part", move |params, context| {
        let choice = choice.clone();
        async move { choice.handle(params, context).await }
    })?;
    Ok(service)
}

#[cfg(test)]
#[path = "../tests/support/wire.rs"]
mod wire;

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::{
        ClientHandler,
        model::ClientConfig,
        service::{ClientLifecycleMode, ClientServiceExt},
    };
    use serde_json::Value;

    struct AppHost;
    impl ClientHandler for AppHost {
        fn get_info(&self) -> ClientConfig {
            let mut config = ClientConfig::default();
            config.capabilities = serde_json::from_value(json!({
                "extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":[APP_MIME_TYPE]}}
            }))
            .unwrap();
            config
        }
    }

    // Exercise the shipped example itself, rather than another fixture server.
    // This proves wire declarations and routing, not rendering in an OpenAI host.
    #[tokio::test]
    async fn example_modern_part_choice_consumes_its_state() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (server_io, client_io) = tokio::io::duplex(65536);
            let inner = Server::new().unwrap();
            let choices = inner.choice.clone();
            let server = tokio::spawn(service(inner).unwrap().serve(server_io));
            let mut client = crate::wire::Rpc::new(client_io);
            client.modern(1, "server/discover", json!({})).await;
            let server = server.await.unwrap().unwrap();
            let initial = client.modern(2, "tools/call", json!({"name":"choose_part","arguments":{}})).await;
            assert_eq!(initial["result"]["resultType"], "input_required");
            assert_eq!(initial["result"]["inputRequests"]["answer"]["method"], "openai/elicitation/create");
            let retry = json!({"name":"choose_part","arguments":{},"requestState":initial["result"]["requestState"],"inputResponses":{"answer":{"action":"accept","content":{"part":"bolt"}}}});
            let complete = client.modern(3,"tools/call",retry.clone()).await;
            assert_eq!(complete["result"]["resultType"], "complete");
            assert_eq!(complete["result"]["structuredContent"], json!({"action":"accept","content":{"part":"bolt"}}));
            assert!(client.modern(4,"tools/call",retry).await.get("error").is_some());
            let initial = client.modern(5, "tools/call", json!({"name":"choose_part","arguments":{}})).await;
            let state = initial["result"]["requestState"].as_str().unwrap();
            // Exercise the example's actual expiry without a ten-minute wait.
            choices.pending.lock().await.get_mut(state).unwrap().expires = Instant::now() - Duration::from_secs(1);
            let expired = client.modern(6, "tools/call", json!({"name":"choose_part","arguments":{},"requestState":state,"inputResponses":{"answer":{"action":"accept","content":{"part":"bolt"}}}})).await;
            assert_eq!(expired["error"]["message"], "choice state expired");
            assert!(!choices.pending.lock().await.contains_key(state));
            server.cancel().await.unwrap();
        }).await.unwrap();
    }

    #[tokio::test]
    async fn example_launch_settings_and_resources_work_on_both_protocol_profiles() {
        for lifecycle in [
            ClientLifecycleMode::Initialize,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        ] {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let (server_io, client_io) = tokio::io::duplex(65536);
                let server =
                    tokio::spawn(service(Server::new().unwrap()).unwrap().serve(server_io));
                let client = AppHost
                    .serve_with_lifecycle(client_io, lifecycle)
                    .await
                    .unwrap();
                let server = server.await.unwrap().unwrap();
                let capability =
                    serde_json::to_value(&client.peer_info().unwrap().capabilities).unwrap();
                assert_eq!(
                    capability["extensions"]["openai/settings"],
                    json!({"readTool":"settings.read","updateTool":"settings.update"})
                );
                let tools = client.list_all_tools().await.unwrap();
                let view = tools.iter().find(|t| t.name == "view_parts").unwrap();
                assert_eq!(view.title.as_deref(), Some("Parts Tray"));
                assert_eq!(view.meta.as_ref().unwrap().0["ui"]["resourceUri"], APP_URI);
                assert_eq!(
                    view.meta.as_ref().unwrap().0["openai/ui"]["entrypoints"],
                    json!([
                        {"type":"global"},{"type":"thread"},{"type":"file","extensions":[".stl"]}
                    ])
                );
                for arguments in [
                    json!({}),
                    json!({"file":{"name":"bolt.stl","resourceUri":"file:///host/bolt.stl"}}),
                ] {
                    let result = client
                        .call_tool(
                            CallToolRequestParams::new("view_parts")
                                .with_arguments(arguments.as_object().unwrap().clone()),
                        )
                        .await
                        .unwrap();
                    assert_eq!(
                        result.structured_content.unwrap()["file"],
                        arguments.get("file").cloned().unwrap_or(Value::Null)
                    );
                }
                assert!(
                    client
                        .call_tool(
                            CallToolRequestParams::new("view_parts").with_arguments(
                                json!({"file":{"name":"","resourceUri":"file:///host/bolt.stl"}})
                                    .as_object()
                                    .unwrap()
                                    .clone()
                            )
                        )
                        .await
                        .is_err()
                );
                let result = client
                    .call_tool(CallToolRequestParams::new("settings.read"))
                    .await
                    .unwrap()
                    .structured_content
                    .unwrap();
                let schema = tools
                    .iter()
                    .find(|t| t.name == "settings.read")
                    .unwrap()
                    .output_schema
                    .as_ref()
                    .unwrap();
                let validator = jsonschema::validator_for(&json!(schema)).unwrap();
                validator.validate(&result).unwrap();
                for changed in [
                    {
                        let mut result = result.clone();
                        result["schema"] = json!({});
                        result
                    },
                    {
                        let mut result = result.clone();
                        result["layout"][0]["items"] = json!([42]);
                        result
                    },
                    {
                        let mut result = result.clone();
                        result["values"].as_object_mut().unwrap().remove("showGrid");
                        result
                    },
                ] {
                    assert!(!validator.is_valid(&changed));
                }
                let resources = client.list_all_resources().await.unwrap();
                assert!(
                    resources
                        .iter()
                        .any(|r| r.uri == APP_URI && r.mime_type.as_deref() == Some(APP_MIME_TYPE))
                );
                let app = client
                    .read_resource(ReadResourceRequestParams::new(APP_URI))
                    .await
                    .unwrap();
                let content = serde_json::to_value(&app.contents[0]).unwrap();
                assert_eq!(content["uri"], APP_URI);
                assert_eq!(content["mimeType"], APP_MIME_TYPE);
                assert_eq!(content["text"], APP_HTML);
                assert_eq!(
                    content["_meta"]["ui"]["csp"],
                    json!({"connectDomains":[],"resourceDomains":[]})
                );
                assert_eq!(
                    content["_meta"]["openai/ui"]["availableDisplayModes"],
                    json!(["inline", "fullscreen"])
                );
                for representation in ["text", "blob"] {
                    let request = ReadResourceRequestParams::new(DATA_URI).with_meta(
                        serde_json::from_value(
                            json!({"openai/resource":{"representation":representation}}),
                        )
                        .unwrap(),
                    );
                    let result = client.read_resource(request).await.unwrap();
                    let content = serde_json::to_value(&result.contents[0]).unwrap();
                    let expected = if representation == "text" {
                        SUMMARY.to_owned()
                    } else {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD.encode(SUMMARY)
                    };
                    assert_eq!(content[representation], expected);
                }
                client.cancel().await.unwrap();
                server.cancel().await.unwrap();
            })
            .await
            .unwrap();
        }
    }
}
