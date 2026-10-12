use openai_mcp_extensions::{
    OpenAIExtensions, OpenAIRequestContextExt,
    mentions::{self, MentionItem, MentionResource, MentionSearchParams, MentionSearchResult},
    settings::SettingsRegistration,
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
    },
    model::{
        CallToolRequestParams, CallToolResponse, JsonObject, ListToolsResult, ProtocolVersion,
        ServerConfig,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
#[path = "support/wire.rs"]
mod wire;

enum Registry {
    Router(ToolRouter<Server>),
    Custom(BTreeMap<String, ToolRoute<Server>>),
}
struct Server {
    registry: Registry,
    info: ServerConfig,
}
impl Server {
    fn new(custom: bool, calls: Arc<AtomicUsize>, writes: Arc<AtomicUsize>) -> Self {
        let registration = SettingsRegistration::new(json!({"type":"object","properties":{
            "count":{"type":"integer","minimum":1,"title":"Count"}
        }}))
        .unwrap();
        let mut info = ServerConfig::default();
        info.server_info.name = "integration-test".into();
        info.capabilities.extensions = Some(
            [(
                String::from("example/keep"),
                json!({"value":true}).as_object().unwrap().clone(),
            )]
            .into(),
        );
        let read = |_| async { Ok(json!({"count":1}).as_object().unwrap().clone()) };
        let update = move |set: JsonObject, context: RequestContext<RoleServer>| {
            let writes = writes.clone();
            async move {
                assert_eq!(
                    context.openai_client().unwrap().session_id(),
                    Some("synthetic-conversation")
                );
                writes.fetch_add(1, Ordering::SeqCst);
                if set["count"] == 13 {
                    Ok(JsonObject::new())
                } else {
                    Ok(set)
                }
            }
        };
        let mention = move |params: MentionSearchParams, context: RequestContext<RoleServer>| {
            let calls = calls.clone();
            async move {
                assert_eq!(
                    context.openai_client().unwrap().session_id(),
                    Some("synthetic-conversation")
                );
                assert!(!context.ct.is_cancelled());
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(if params.query == "invalid-result" {
                    MentionSearchResult {
                        items: vec![MentionItem::Resource(MentionResource {
                            resource_uri: "example://entry".into(),
                            title: "".into(),
                            subtitle: None,
                            icons: None,
                        })],
                    }
                } else {
                    MentionSearchResult::default()
                })
            }
        };
        let registry = if custom {
            let mut tools = BTreeMap::new();
            let route = mentions::search_tool(mention);
            tools.insert(route.name().to_owned(), route);
            let (routes, capability) = registration.into_tools(read, update).into_parts();
            assert!(routes.iter().all(|route| !tools.contains_key(route.name())));
            for route in routes {
                tools.insert(route.name().to_owned(), route);
            }
            capability.apply_capabilities(&mut info.capabilities);
            mentions::apply_capabilities(&mut info.capabilities);
            Registry::Custom(tools)
        } else {
            let mut router = ToolRouter::new();
            let mut extensions = OpenAIExtensions::new();
            extensions.register_mentions(&mut router, mention).unwrap();
            extensions
                .register_settings(&mut router, registration, read, update)
                .unwrap();
            extensions.apply_capabilities(&mut info.capabilities);
            Registry::Router(router)
        };
        Self { registry, info }
    }
}
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        self.info.clone()
    }
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![
            ProtocolVersion::V_2026_07_28,
            ProtocolVersion::V_2025_11_25,
        ])
    }
    async fn list_tools(
        &self,
        _: Option<rmcp::model::PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = match &self.registry {
            Registry::Router(router) => router.list_all(),
            Registry::Custom(tools) => tools.values().map(|route| route.attr.clone()).collect(),
        };
        Ok(ListToolsResult::with_all_items(tools))
    }
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match &self.registry {
            Registry::Router(router) => {
                router
                    .call(ToolCallContext::new(self, params, context))
                    .await
            }
            Registry::Custom(tools) => {
                let route = tools
                    .get(params.name.as_ref())
                    .ok_or_else(|| ErrorData::invalid_params("tool not found", None))?;
                (route.call)(ToolCallContext::new(self, params, context)).await
            }
        }
    }
}

async fn observe(custom: bool, modern: bool) -> Vec<Value> {
    let calls = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let (server_io, client_io) = tokio::io::duplex(65536);
    let task = tokio::spawn(Server::new(custom, calls.clone(), writes.clone()).serve(server_io));
    let mut rpc = wire::Rpc::new(client_io);
    let discovery = if modern {
        rpc.modern(1, "server/discover", json!({})).await
    } else {
        let response = rpc.request(1, "initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test-host","version":"1"}})).await;
        rpc.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        response
    };
    let server = task.await.unwrap().unwrap();
    let mut observations = vec![discovery];
    let tools = if modern {
        rpc.modern(2, "tools/list", json!({})).await
    } else {
        rpc.request(2, "tools/list", json!({})).await
    };
    observations.push(tools);
    for (index, (name, arguments, error)) in [
        ("search_mentions", json!({"query":""}), false),
        ("search_mentions", json!({"query":42}), true),
        ("search_mentions", json!({"query":"invalid-result"}), true),
        ("settings.read", json!({}), false),
        ("settings.read", json!({"extra":true}), true),
        ("settings.update", json!({"set":{}}), true),
        ("settings.update", json!({"set":{"count":0}}), true),
        (
            "settings.update",
            json!({"set":{"count":2},"extra":true}),
            true,
        ),
        ("settings.update", json!({"set":{"count":2}}), false),
        ("settings.update", json!({"set":{"count":13}}), true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut params = json!({"name":name,"arguments":arguments});
        params["_meta"] = if modern { wire::meta() } else { json!({}) };
        params["_meta"]["openai/session"] = json!("synthetic-conversation");
        let response = rpc.request(3 + index as u64, "tools/call", params).await;
        assert_eq!(response.get("error").is_some(), error, "{name}: {response}");
        observations.push(response);
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "malformed mention input must not run the callback"
    );
    assert_eq!(
        writes.load(Ordering::SeqCst),
        2,
        "invalid patches must not reach persistence; bad results cannot undo a callback"
    );
    server.cancel().await.unwrap();
    observations
}

#[tokio::test]
async fn custom_dispatch_and_router_have_identical_descriptions_validation_and_replies() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        for modern in [false, true] {
            let router = observe(false, modern).await;
            let custom = observe(true, modern).await;
            assert_eq!(custom, router);
            assert_eq!(
                custom[0]["result"]["capabilities"]["extensions"]["example/keep"],
                json!({"value":true})
            );
            assert_eq!(
                custom[0]["result"]["capabilities"]["extensions"]["openai/settings"],
                json!({"readTool":"settings.read","updateTool":"settings.update"})
            );
            assert_eq!(
                custom[2]["result"]["structuredContent"],
                json!({"items":[]})
            );
            assert_eq!(
                custom.iter().find(|response| response["id"] == 11).unwrap()["result"]["structuredContent"],
                json!({"values":{"count":2}})
            );
        }
    })
    .await
    .unwrap();
}
