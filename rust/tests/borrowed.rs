use openai_mcp_extensions::{
    Error, ExtensionFuture, OpenAIExtensions, OpenAIRequestContextExt,
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
        CallToolRequestParams, CallToolResponse, JsonObject, ListToolsResult, RequestId,
        ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Mutex;

#[path = "support/wire.rs"]
mod wire;

enum Registry {
    Router(ToolRouter<Server>),
    Custom(BTreeMap<String, ToolRoute<Server>>),
}

// State belongs to the consumer's server. Its callbacks capture no cloned Arc.
struct Server {
    registry: Registry,
    info: ServerConfig,
    label: String,
    values: Mutex<JsonObject>,
    searches: AtomicUsize,
    reads: AtomicUsize,
    writes: AtomicUsize,
}

#[derive(Clone)]
struct OriginalRequest(RequestId);

fn assert_context(context: &RequestContext<RoleServer>) {
    assert_eq!(
        context.extensions.get::<OriginalRequest>().unwrap().0,
        context.id
    );
    assert_eq!(
        context.meta.get("test/keep"),
        Some(&json!({"trace":"original context"}))
    );
    assert_eq!(
        context.openai_client().unwrap().session_id(),
        Some("borrowed-test")
    );
    assert!(!context.ct.is_cancelled());
}

fn search<'a>(
    server: &'a Server,
    params: MentionSearchParams,
    context: RequestContext<RoleServer>,
) -> ExtensionFuture<'a, MentionSearchResult> {
    Box::pin(async move {
        tokio::task::yield_now().await;
        assert_context(&context);
        server.searches.fetch_add(1, Ordering::SeqCst);
        if params.query == "callback-error" {
            return Err(ErrorData::invalid_request("consumer denied search", None));
        }
        Ok(MentionSearchResult {
            items: vec![MentionItem::Resource(MentionResource {
                resource_uri: "example://borrowed".into(),
                title: if params.query == "invalid-result" {
                    String::new()
                } else {
                    server.label.clone()
                },
                subtitle: None,
                icons: None,
            })],
        })
    })
}

fn read<'a>(
    server: &'a Server,
    context: RequestContext<RoleServer>,
) -> ExtensionFuture<'a, JsonObject> {
    Box::pin(async move {
        tokio::task::yield_now().await;
        assert_context(&context);
        server.reads.fetch_add(1, Ordering::SeqCst);
        Ok(server.values.lock().await.clone())
    })
}

fn update<'a>(
    server: &'a Server,
    set: JsonObject,
    context: RequestContext<RoleServer>,
) -> ExtensionFuture<'a, JsonObject> {
    Box::pin(async move {
        tokio::task::yield_now().await;
        assert_context(&context);
        let mut values = server.values.lock().await;
        server.writes.fetch_add(1, Ordering::SeqCst);
        values.extend(set);
        if values["count"] == 13 {
            Ok(JsonObject::new())
        } else {
            Ok(values.clone())
        }
    })
}

fn registration() -> SettingsRegistration {
    SettingsRegistration::new(json!({"type":"object","properties":{
        "count":{"type":"integer","minimum":1,"title":"Count"},
        "unit":{"type":"string","enum":["mm","in"],"title":"Unit"}
    }}))
    .unwrap()
}

impl Server {
    fn new(custom: bool) -> Self {
        let mut info = ServerConfig::default();
        info.server_info.name = "borrowed-state-test".into();
        info.capabilities.extensions = Some(
            [(
                String::from("example/keep"),
                json!({"value":true}).as_object().unwrap().clone(),
            )]
            .into(),
        );
        let registry = if custom {
            let mut tools = BTreeMap::new();
            let route = mentions::search_tool_with_state(search);
            tools.insert(route.name().to_owned(), route);
            let (routes, capability) = registration()
                .into_tools_with_state(read, update)
                .into_parts();
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
            extensions
                .register_mentions_with_state(&mut router, search)
                .unwrap();
            extensions
                .register_settings_with_state(&mut router, registration(), read, update)
                .unwrap();
            extensions.apply_capabilities(&mut info.capabilities);
            Registry::Router(router)
        };
        Self {
            registry,
            info,
            label: "Server-owned catalog".into(),
            values: Mutex::new(json!({"count":1,"unit":"mm"}).as_object().unwrap().clone()),
            searches: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        }
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        self.info.clone()
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
        mut context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Consumer middleware can add request data before SDK dispatch.
        context
            .extensions
            .insert(OriginalRequest(context.id.clone()));
        let context = ToolCallContext::new(self, params, context);
        match &self.registry {
            Registry::Router(router) => router.call(context).await,
            Registry::Custom(tools) => {
                let route = tools
                    .get(context.name())
                    .ok_or_else(|| ErrorData::invalid_params("tool not found", None))?;
                (route.call)(context).await
            }
        }
    }
}

async fn observe(custom: bool) -> Vec<Value> {
    let (server_io, client_io) = tokio::io::duplex(65536);
    let task = tokio::spawn(Server::new(custom).serve(server_io));
    let mut rpc = wire::Rpc::new(client_io);
    let mut observations = vec![rpc.modern(1, "server/discover", json!({})).await];
    let server = task.await.unwrap().unwrap();
    observations.push(rpc.modern(2, "tools/list", json!({})).await);
    for (index, (name, arguments, error_code)) in [
        ("search_mentions", json!({"query":""}), None),
        ("search_mentions", json!({"query":42}), Some(-32602)),
        (
            "search_mentions",
            json!({"query":"invalid-result"}),
            Some(-32603),
        ),
        (
            "search_mentions",
            json!({"query":"callback-error"}),
            Some(-32600),
        ),
        ("settings.read", json!({}), None),
        ("settings.read", json!({"extra":true}), Some(-32602)),
        ("settings.update", json!({"set":{}}), Some(-32602)),
        ("settings.update", json!({"set":{"count":0}}), Some(-32602)),
        (
            "settings.update",
            json!({"set":{"count":2},"extra":true}),
            Some(-32602),
        ),
        (
            "settings.update",
            json!({"set":{"unknown":true}}),
            Some(-32602),
        ),
        ("settings.update", json!({"set":{"count":2}}), None),
        ("settings.update", json!({"set":{"count":13}}), Some(-32603)),
        ("settings.read", json!({}), None),
    ]
    .into_iter()
    .enumerate()
    {
        let mut meta = wire::meta();
        meta["openai/session"] = json!("borrowed-test");
        let response = rpc
            .request(
                3 + index as u64,
                "tools/call",
                json!({
                    "name":name,"arguments":arguments,"_meta":meta
                }),
            )
            .await;
        assert_eq!(
            response["error"]["code"].as_i64(),
            error_code,
            "{name}: {response}"
        );
        observations.push(response);
    }
    assert_eq!(
        server.service().searches.load(Ordering::SeqCst),
        3,
        "malformed input must not invoke search"
    );
    assert_eq!(
        server.service().reads.load(Ordering::SeqCst),
        2,
        "nonempty read arguments must not invoke read"
    );
    assert_eq!(
        server.service().writes.load(Ordering::SeqCst),
        2,
        "invalid patches must not reach consumer writes"
    );
    assert_eq!(
        server.service().values.lock().await["count"],
        13,
        "result validation cannot roll back a consumer write"
    );
    server.cancel().await.unwrap();
    observations
}

#[tokio::test]
async fn borrowed_state_survives_await_in_custom_and_router_dispatch() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let router = observe(false).await;
        let custom = observe(true).await;
        assert_eq!(
            custom, router,
            "declarations, capabilities and wire behavior must match"
        );
        assert_eq!(
            custom[0]["result"]["capabilities"]["extensions"]["example/keep"],
            json!({"value":true})
        );
        assert_eq!(
            custom[2]["result"]["structuredContent"]["items"][0]["title"],
            "Server-owned catalog"
        );
        assert_eq!(
            custom[12]["result"]["structuredContent"],
            json!({"values":{"count":2,"unit":"mm"}})
        );
        assert_eq!(
            custom[14]["result"]["structuredContent"]["values"],
            json!({"count":13,"unit":"mm"})
        );
    })
    .await
    .unwrap();
}

#[test]
fn borrowed_registration_preserves_enabled_disabled_and_reserved_collisions() {
    for name in ["search_mentions", "settings.read", "settings.update"] {
        for state in ["enabled", "disabled", "reserved"] {
            let mut router: ToolRouter<()> = ToolRouter::new();
            if state != "reserved" {
                router.add_route(ToolRoute::new_dyn(
                    Tool::new(name, "Keep this consumer route", JsonObject::new()),
                    |_| Box::pin(async { unreachable!() }),
                ));
            }
            if state != "enabled" {
                router.disable_route(name);
            }
            let mut extensions = OpenAIExtensions::new();
            let result = if name == "search_mentions" {
                extensions.register_mentions_with_state(&mut router, |_, _, _| {
                    Box::pin(async { Ok(MentionSearchResult::default()) })
                })
            } else {
                extensions.register_settings_with_state(
                    &mut router,
                    registration(),
                    |_, _| Box::pin(async { Ok(JsonObject::new()) }),
                    |_, _, _| Box::pin(async { Ok(JsonObject::new()) }),
                )
            };
            assert!(matches!(result, Err(Error::ToolCollision(ref actual)) if actual == name));
            assert_eq!(router.map.len(), usize::from(state != "reserved"));
            if state != "reserved" {
                assert_eq!(
                    router.map[name].attr.description.as_deref(),
                    Some("Keep this consumer route")
                );
            }
            let mut capabilities = Default::default();
            extensions.apply_capabilities(&mut capabilities);
            assert!(capabilities.tools.is_none());
            assert!(capabilities.extensions.is_none());
            assert!(capabilities.experimental.is_none());
            if state != "enabled" {
                assert!(router.enable_route(name));
            }
        }
    }
}

#[test]
fn owned_and_borrowed_builders_keep_identical_declarations() {
    let owned =
        mentions::search_tool::<(), _, _>(|_, _| async { Ok(MentionSearchResult::default()) });
    let borrowed = mentions::search_tool_with_state(|_: &(), _, _| {
        Box::pin(async { Ok(MentionSearchResult::default()) })
    });
    assert_eq!(owned.attr, borrowed.attr);
    let owned = registration().into_tools::<(), _, _, _, _>(
        |_| async { Ok(JsonObject::new()) },
        |_, _| async { Ok(JsonObject::new()) },
    );
    let borrowed = registration().into_tools_with_state(
        |_: &(), _| Box::pin(async { Ok(JsonObject::new()) }),
        |_: &(), _, _| Box::pin(async { Ok(JsonObject::new()) }),
    );
    assert_eq!(owned.read().attr, borrowed.read().attr);
    assert_eq!(owned.update().attr, borrowed.update().attr);
    assert_eq!(owned.capability(), borrowed.capability());
}
