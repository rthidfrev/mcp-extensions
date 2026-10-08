use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use openai_mcp_extensions::{
    Error, OpenAIExtensions,
    forms::{self, OpenAIForm},
    mentions::MentionSearchResult,
    settings::SettingsRegistration,
};
use rmcp::{
    ClientHandler, ErrorData, RoleClient, RoleServer, ServerHandler, ServiceExt,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
    },
    model::{
        CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientConfig, CustomRequest,
        CustomResult, ListToolsResult, ProtocolVersion, ServerConfig, Tool,
    },
    service::{ClientLifecycleMode, ClientServiceExt, RequestContext},
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

struct Server {
    router: ToolRouter<Self>,
    extensions: OpenAIExtensions,
    version: ProtocolVersion,
}

impl Server {
    fn new(version: ProtocolVersion, writes: Arc<AtomicUsize>) -> Self {
        let mut router = ToolRouter::new();
        let mut extensions = OpenAIExtensions::new();
        extensions
            .register_mentions(&mut router, |params, _| async move {
                assert_eq!(params.query, "");
                Ok(MentionSearchResult::default())
            })
            .unwrap();
        let values = Arc::new(Mutex::new(
            json!({"enabled":true,"count":1})
                .as_object()
                .unwrap()
                .clone(),
        ));
        let read = values.clone();
        extensions.register_settings(&mut router, SettingsRegistration::new(json!({"type":"object","properties":{"enabled":{"type":"boolean","title":"Enabled"},"count":{"type":"integer","title":"Count","minimum":1}}})).unwrap(),
            move |_| { let values = read.clone(); async move { Ok(values.lock().await.clone()) } },
            move |set, _| { let values = values.clone(); let writes = writes.clone(); async move { writes.fetch_add(1,Ordering::SeqCst); let mut values = values.lock().await; values.extend(set); Ok(values.clone()) } }
        ).unwrap();
        let form = Arc::new(OpenAIForm::from_json(form_schema()).unwrap());
        router.add_route(ToolRoute::new_dyn(
            Tool::new(
                "form",
                "OpenAI form",
                json!({"type":"object"}).as_object().unwrap().clone(),
            ),
            move |context| {
                let form = form.clone();
                Box::pin(async move {
                    let result =
                        forms::elicit_input(&context.request_context, "Choose a code", &form)
                            .await
                            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                    Ok(rmcp::model::CallToolResult::structured(
                        serde_json::to_value(result).unwrap(),
                    )
                    .into())
                })
            },
        ));
        Self {
            router,
            extensions,
            version,
        }
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::default();
        config.capabilities.extensions = Some(
            [(
                String::from("example/other"),
                json!({"keep":true}).as_object().unwrap().clone(),
            )]
            .into(),
        );
        self.extensions.apply_capabilities(&mut config.capabilities);
        config
    }
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![self.version.clone()])
    }
    async fn list_tools(
        &self,
        _: Option<rmcp::model::PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.router.list_all()))
    }
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.router
            .call(ToolCallContext::new(self, params, context))
            .await
    }
}

struct Host {
    response: Value,
    capability: Option<Value>,
    requests: Arc<Mutex<Vec<CustomRequest>>>,
}
impl ClientHandler for Host {
    fn get_info(&self) -> ClientConfig {
        let mut config = ClientConfig::default();
        if let Some(capability) = &self.capability {
            config.capabilities =
                serde_json::from_value(json!({"extensions":{"openai/elicitation":capability}}))
                    .unwrap();
        }
        config
    }
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _: RequestContext<RoleClient>,
    ) -> Result<CustomResult, ErrorData> {
        self.requests.lock().await.push(request);
        Ok(CustomResult::new(self.response.clone()))
    }
}
fn form_schema() -> Value {
    json!({"type":"object","properties":{"code":{"type":"string","pattern":"^[A-Z]{2}$","x-openai-suggestions":[{"const":"AB","title":"A hint","x-openai-thumbnail":{"src":"https://example.com/a.png"}}]},"tags":{"type":"array","items":{"type":"string"}}},"required":["code","tags"]})
}
fn args(name: &str, value: Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name.to_owned()).with_arguments(value.as_object().unwrap().clone())
}

#[tokio::test]
async fn discovery_and_settings_work_over_a_real_mcp_transport() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let writes = Arc::new(AtomicUsize::new(0));
        let (server_io, client_io) = tokio::io::duplex(65536);
        let server = tokio::spawn(
            Server::new(ProtocolVersion::V_2025_11_25, writes.clone()).serve(server_io),
        );
        let client = Host {
            response: json!({"action":"cancel"}),
            capability: None,
            requests: Default::default(),
        }
        .serve(client_io)
        .await
        .unwrap();
        let server = server.await.unwrap().unwrap();
        let info = client.peer_info().unwrap();
        let caps = serde_json::to_value(&info.capabilities).unwrap();
        assert_eq!(
            caps["extensions"]["openai/settings"],
            json!({"readTool":"settings.read","updateTool":"settings.update"})
        );
        assert_eq!(
            caps["experimental"]["openai/settings"],
            caps["extensions"]["openai/settings"]
        );
        assert_eq!(caps["extensions"]["example/other"], json!({"keep":true}));
        let tools = client.list_all_tools().await.unwrap();
        let mentions = tools.iter().find(|t| t.name == "search_mentions").unwrap();
        assert_eq!(
            mentions.annotations.as_ref().unwrap().read_only_hint,
            Some(true)
        );
        assert_eq!(
            mentions.meta.as_ref().unwrap().0["ui"],
            json!({"visibility":["app"]})
        );
        assert_eq!(
            mentions.meta.as_ref().unwrap().0["openai/extensions"],
            json!({"mentions/search":{}})
        );
        let mentions_result = client
            .call_tool(args("search_mentions", json!({"query":""})))
            .await
            .unwrap();
        assert!(mentions_result.content.is_empty());
        assert_eq!(
            mentions_result.structured_content,
            Some(json!({"items":[]}))
        );
        let result = client
            .call_tool(args("settings.read", json!({})))
            .await
            .unwrap();
        assert_eq!(
            result.structured_content.as_ref().unwrap()["values"],
            json!({"enabled":true,"count":1})
        );
        for bad in [
            json!({"set":{}}),
            json!({"set":{"count":0}}),
            json!({"set":{"missing":false}}),
            json!({"set":{"enabled":false},"extra":true}),
        ] {
            assert!(
                client
                    .call_tool(args("settings.update", bad))
                    .await
                    .is_err()
            );
        }
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        let result = client
            .call_tool(args("settings.update", json!({"set":{"enabled":false}})))
            .await
            .unwrap();
        assert_eq!(
            result.structured_content.as_ref().unwrap()["values"],
            json!({"enabled":false,"count":1})
        );
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        let output = tools
            .iter()
            .find(|t| t.name == "settings.update")
            .unwrap()
            .output_schema
            .as_ref()
            .unwrap();
        jsonschema::validator_for(&json!(output))
            .unwrap()
            .validate(result.structured_content.as_ref().unwrap())
            .unwrap();
        client.cancel().await.unwrap();
        server.cancel().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn direct_forms_preserve_wire_fields_and_validate_peer_answers() {
    for (response, success) in [
        (
            json!({"action":"accept","content":{"code":"AB","tags":["a","b"]}}),
            true,
        ),
        (json!({"action":"cancel"}), true),
        (json!({"action":"decline"}), true),
        (
            json!({"action":"accept","content":{"code":"bad","tags":[]}}),
            false,
        ),
        (json!({"action":"accept","content":{}}), false),
        (json!({"action":"unknown"}), false),
    ] {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let requests = Arc::new(Mutex::new(vec![]));
            let (server_io, client_io) = tokio::io::duplex(65536);
            let server = tokio::spawn(
                Server::new(ProtocolVersion::V_2025_11_25, Default::default()).serve(server_io),
            );
            let client = Host {
                response: response.clone(),
                capability: Some(json!({"form":{}})),
                requests: requests.clone(),
            }
            .serve(client_io)
            .await
            .unwrap();
            let server = server.await.unwrap().unwrap();
            let result = client.call_tool(args("form", json!({}))).await;
            if success {
                assert_eq!(result.unwrap().structured_content, Some(response));
            } else {
                assert!(result.is_err());
            }
            let requests = requests.lock().await;
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].method, "openai/elicitation/create");
            assert_eq!(
                requests[0].params,
                Some(
                    json!({"mode":"form","message":"Choose a code","requestedSchema":form_schema()})
                )
            );
            client.cancel().await.unwrap();
            server.cancel().await.unwrap();
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn unsupported_clients_and_modern_protocol_never_receive_direct_requests() {
    for (version, capability) in [
        (ProtocolVersion::V_2025_11_25, None),
        (ProtocolVersion::V_2025_11_25, Some(json!({"form":true}))),
        (ProtocolVersion::V_2026_07_28, Some(json!({"form":{}}))),
    ] {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let requests = Arc::new(Mutex::new(vec![]));
            let (server_io, client_io) = tokio::io::duplex(65536);
            let lifecycle = if version == ProtocolVersion::V_2026_07_28 {
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![version.clone()],
                }
            } else {
                ClientLifecycleMode::Initialize
            };
            let server = tokio::spawn(Server::new(version, Default::default()).serve(server_io));
            let client = Host {
                response: json!({"action":"cancel"}),
                capability,
                requests: requests.clone(),
            }
            .serve_with_lifecycle(client_io, lifecycle)
            .await
            .unwrap();
            let server = server.await.unwrap().unwrap();
            assert!(client.call_tool(args("form", json!({}))).await.is_err());
            assert!(requests.lock().await.is_empty());
            client.cancel().await.unwrap();
            server.cancel().await.unwrap();
        })
        .await
        .unwrap();
    }
}

#[test]
fn collisions_do_not_overwrite_routes_or_half_register_settings() {
    let mut router: ToolRouter<()> = ToolRouter::new();
    let mut extensions = OpenAIExtensions::new();
    extensions
        .register_mentions(&mut router, |_, _| async {
            Ok(MentionSearchResult::default())
        })
        .unwrap();
    assert!(matches!(
        extensions.register_mentions(&mut router, |_, _| async {
            Ok(MentionSearchResult::default())
        }),
        Err(Error::ToolCollision(_))
    ));
    let registration = SettingsRegistration::new(json!({"type":"object","properties":{}}))
        .unwrap()
        .with_tool_names("settings.read", "search_mentions")
        .unwrap();
    assert!(
        extensions
            .register_settings(
                &mut router,
                registration,
                |_| async { Ok(Default::default()) },
                |_, _| async { Ok(Default::default()) }
            )
            .is_err()
    );
    assert!(!router.has_route("settings.read"));
    assert_eq!(router.list_all().len(), 1);
    let mut caps = rmcp::model::ServerCapabilities::default();
    extensions.apply_capabilities(&mut caps);
    assert!(caps.extensions.is_none());
    assert!(!forms::supports_forms(&ClientCapabilities::default()));
}

#[test]
fn disabled_names_are_reserved_without_partial_installation_or_notifications() {
    for name in ["search_mentions", "settings.read", "settings.update"] {
        for registered in [false, true] {
            let mut router: ToolRouter<()> = ToolRouter::new();
            if registered {
                router.add_route(ToolRoute::new_dyn(
                    Tool::new(name, "Keep this tool", rmcp::model::JsonObject::new()),
                    |_| Box::pin(async { unreachable!() }),
                ));
            }
            router.disable_route(name);
            let notices = Arc::new(AtomicUsize::new(0));
            let observed = notices.clone();
            router.set_notifier(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            });
            let mut extensions = OpenAIExtensions::new();
            let result = if name == "search_mentions" {
                extensions.register_mentions(&mut router, |_, _| async {
                    Ok(MentionSearchResult::default())
                })
            } else {
                extensions.register_settings(
                    &mut router,
                    SettingsRegistration::new(json!({"type":"object","properties":{}})).unwrap(),
                    |_| async { Ok(Default::default()) },
                    |_, _| async { Ok(Default::default()) },
                )
            };
            assert!(matches!(result, Err(Error::ToolCollision(ref actual)) if actual == name));
            assert_eq!(router.map.len(), usize::from(registered));
            if registered {
                assert_eq!(
                    router.map[name].attr.description.as_deref(),
                    Some("Keep this tool")
                );
                assert!(router.is_disabled(name));
            }
            assert!(router.list_all().is_empty());
            assert_eq!(notices.load(Ordering::SeqCst), 0);
            let mut capabilities = rmcp::model::ServerCapabilities::default();
            extensions.apply_capabilities(&mut capabilities);
            assert!(capabilities.tools.is_none());
            assert!(capabilities.extensions.is_none());
            assert!(capabilities.experimental.is_none());
            assert!(
                router.enable_route(name),
                "the reserved disabled marker must survive"
            );
        }
    }
}
