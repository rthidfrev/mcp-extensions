use openai_mcp_extensions::{
    Error, ExtensionFuture, OpenAIExtensions, OpenAIRequestContextExt,
    model::{FieldType, Model, ModelField},
    settings::{SettingsGroup, SettingsLayoutItem, SettingsModel, TypedSettings},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
    },
    model::{CallToolRequestParams, CallToolResponse, JsonObject, ListToolsResult, ServerConfig},
    service::RequestContext,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::Mutex;

#[path = "support/wire.rs"]
mod wire;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Values {
    enabled: bool,
    // An intentionally incorrect consumer serializer tests complete-result checks.
    #[serde(skip_serializing_if = "bad_count")]
    count: u8,
    #[serde(rename = "display/name~")]
    label: String,
}
fn bad_count(value: &u8) -> bool {
    *value == 13
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Patch {
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<u8>,
    #[serde(rename = "display/name~", skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}
impl Model for Values {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<bool>("enabled", "Enabled")?,
            ModelField::new::<u8>("count", "Count")?,
            ModelField::new::<String>("display/name~", "Label")?,
        ])
    }
}
impl Model for Patch {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<Option<bool>>("enabled", "Enabled")?,
            ModelField::new::<Option<u8>>("count", "Count")?,
            ModelField::new::<Option<String>>("display/name~", "Label")?,
        ])
    }
}
impl SettingsModel for Values {
    type Patch = Patch;
}

fn initial() -> Values {
    Values {
        enabled: true,
        count: 4,
        label: "retained".into(),
    }
}
fn object(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

#[test]
fn values_only_serialize_and_patches_only_deserialize() {
    #[derive(Serialize)]
    struct ReadValues {
        enabled: bool,
    }
    #[derive(Deserialize)]
    struct WritePatch {
        enabled: Option<bool>,
    }
    impl Model for ReadValues {
        fn fields() -> Result<Vec<ModelField>, Error> {
            Ok(vec![ModelField::new::<bool>("enabled", "Enabled")?])
        }
    }
    impl Model for WritePatch {
        fn fields() -> Result<Vec<ModelField>, Error> {
            Ok(vec![ModelField::new::<Option<bool>>("enabled", "Enabled")?])
        }
    }
    impl SettingsModel for ReadValues {
        type Patch = WritePatch;
    }
    let settings = TypedSettings::<ReadValues>::new().unwrap();
    assert_eq!(
        settings
            .encode_values(&ReadValues { enabled: true })
            .unwrap(),
        object(json!({"enabled":true}))
    );
    assert_eq!(
        settings
            .decode_patch(object(json!({"enabled":false})))
            .unwrap()
            .enabled,
        Some(false)
    );
}

#[test]
fn direct_conversion_preserves_omission_and_falsy_replacements() {
    let settings = TypedSettings::<Values>::new().unwrap();
    let patch = settings.decode_patch(object(json!({"count":0}))).unwrap();
    assert_eq!(
        patch,
        Patch {
            enabled: None,
            count: Some(0),
            label: None
        }
    );
    assert_eq!(serde_json::to_value(patch).unwrap(), json!({"count":0}));
    assert_eq!(
        settings
            .decode_patch(object(json!({"enabled":false,"display/name~":""})))
            .unwrap(),
        Patch {
            enabled: Some(false),
            count: None,
            label: Some(String::new())
        }
    );
    let values = Values {
        enabled: false,
        count: 0,
        label: String::new(),
    };
    assert_eq!(
        settings.encode_values(&values).unwrap(),
        object(json!({"enabled":false,"count":0,"display/name~":""}))
    );
    for (set, path) in [
        (json!({}), ""),
        (json!({"count":null}), "/count"),
        (json!({"count":256}), "/count"),
        (json!({"unknown":1}), "/unknown"),
        (json!({"display/name~":false}), "/display~1name~0"),
    ] {
        assert_eq!(
            settings
                .validate_patch(&object(set.clone()))
                .unwrap_err()
                .path(),
            Some(path)
        );
        assert_eq!(
            settings.decode_patch(object(set)).unwrap_err().path(),
            Some(path)
        );
    }
    let bad = Values {
        count: 13,
        ..initial()
    };
    assert_eq!(
        settings.encode_values(&bad).unwrap_err().path(),
        Some("/count")
    );
}

#[derive(Serialize, Deserialize)]
struct Single<T> {
    value: T,
}
#[derive(Serialize, Deserialize)]
struct Partial<T> {
    value: Option<T>,
}
impl<T: FieldType + DeserializeOwned> Model for Single<T> {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![ModelField::new::<T>("value", "Value")?])
    }
}
impl<T: FieldType + DeserializeOwned> Model for Partial<T> {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![ModelField::new::<Option<T>>("value", "Value")?])
    }
}
impl<T: FieldType + Serialize + DeserializeOwned> SettingsModel for Single<T> {
    type Patch = Partial<T>;
}

#[test]
fn integral_patches_decode_without_inserting_omitted_fields() {
    let settings = TypedSettings::<Values>::new().unwrap();
    for spelling in ["1", "1.0", "1e0"] {
        let set = object(json!({"count":serde_json::from_str::<Value>(spelling).unwrap()}));
        settings.validate_patch(&set).unwrap();
        let patch = settings.decode_patch(set).unwrap();
        assert_eq!(patch.count, Some(1));
        assert_eq!(patch.enabled, None);
        assert_eq!(patch.label, None);
    }
}

#[derive(Serialize)]
struct NumericValues {
    count: u8,
    value: f32,
}
#[derive(Debug, Deserialize)]
struct NumericPatch {
    count: Option<u8>,
    value: Option<f32>,
}
fn numeric_fields<C: FieldType, F: FieldType>() -> Result<Vec<ModelField>, Error> {
    Ok(vec![
        ModelField::new::<C>("count", "Count")?,
        ModelField::new::<F>("value", "Value")?
            .with_annotations(object(json!({"minimum":1e-46})))?,
    ])
}
impl Model for NumericValues {
    fn fields() -> Result<Vec<ModelField>, Error> {
        numeric_fields::<u8, f32>()
    }
}
impl Model for NumericPatch {
    fn fields() -> Result<Vec<ModelField>, Error> {
        numeric_fields::<Option<u8>, Option<f32>>()
    }
}
impl SettingsModel for NumericValues {
    type Patch = NumericPatch;
}
fn numeric_update(patch: NumericPatch) -> NumericValues {
    NumericValues {
        count: patch.count.unwrap_or(0),
        value: patch.value.unwrap_or(1.0),
    }
}

struct NumericServer {
    registry: Registry<Self>,
    info: ServerConfig,
    writes: Arc<AtomicUsize>,
}
impl NumericServer {
    fn new(borrowed: bool, custom: bool) -> Self {
        let settings = TypedSettings::<NumericValues>::new().unwrap();
        let writes = Arc::new(AtomicUsize::new(0));
        let tools = if borrowed {
            settings.into_tools_with_state(
                |_: &Self, _| {
                    Box::pin(async {
                        Ok(numeric_update(NumericPatch {
                            count: None,
                            value: None,
                        }))
                    })
                },
                |server: &Self, patch, _| {
                    Box::pin(async move {
                        tokio::task::yield_now().await;
                        server.writes.fetch_add(1, Ordering::SeqCst);
                        Ok(numeric_update(patch))
                    })
                },
            )
        } else {
            let update_writes = writes.clone();
            settings.into_tools(
                |_| async {
                    Ok(numeric_update(NumericPatch {
                        count: None,
                        value: None,
                    }))
                },
                move |patch, _| {
                    let writes = update_writes.clone();
                    async move {
                        writes.fetch_add(1, Ordering::SeqCst);
                        Ok(numeric_update(patch))
                    }
                },
            )
        };
        let mut info = ServerConfig::default();
        let (routes, capability) = tools.into_parts();
        capability.apply_capabilities(&mut info.capabilities);
        let registry = if custom {
            Registry::Custom(
                routes
                    .into_iter()
                    .map(|route| (route.name().to_owned(), route))
                    .collect(),
            )
        } else {
            let mut router = ToolRouter::new();
            for route in routes {
                router.add_route(route);
            }
            Registry::Router(router)
        };
        Self {
            registry,
            info,
            writes,
        }
    }
}
impl ServerHandler for NumericServer {
    fn get_info(&self) -> ServerConfig {
        self.info.clone()
    }
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let context = ToolCallContext::new(self, params, context);
        match &self.registry {
            Registry::Router(router) => router.call(context).await,
            Registry::Custom(routes) => (routes.get(context.name()).unwrap().call)(context).await,
        }
    }
}

#[tokio::test]
async fn numeric_conversion_is_shared_by_both_registries_and_callback_styles() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let settings = TypedSettings::<NumericValues>::new().unwrap();
        let invalid = object(json!({"value":1e-46}));
        settings.registration().validate_patch(&invalid).unwrap();
        assert_eq!(settings.validate_patch(&invalid).unwrap_err().path(), Some("/value"));
        assert_eq!(settings.decode_patch(invalid).unwrap_err().path(), Some("/value"));
        for borrowed in [false, true] {
            for custom in [false, true] {
                let (server_io, client_io) = tokio::io::duplex(65536);
                let task = tokio::spawn(NumericServer::new(borrowed, custom).serve(server_io));
                let mut rpc = wire::Rpc::new(client_io);
                rpc.modern(1, "server/discover", json!({})).await;
                let server = task.await.unwrap().unwrap();
                let bad = rpc.modern(2, "tools/call", json!({"name":"settings.update","arguments":{"set":{"value":1e-46}}})).await;
                assert_eq!(bad["error"]["code"], -32602);
                assert!(bad["error"]["message"].as_str().unwrap().contains("/value"));
                assert_eq!(server.service().writes.load(Ordering::SeqCst), 0);
                for (index, spelling) in ["1.0", "1e0"].into_iter().enumerate() {
                    let number: Value = serde_json::from_str(spelling).unwrap();
                    let good = rpc.modern(3 + index as u64, "tools/call", json!({"name":"settings.update","arguments":{"set":{"count":number,"value":1}}})).await;
                    assert_eq!(good["result"]["structuredContent"], json!({"values":{"count":1,"value":1.0}}));
                }
                assert_eq!(server.service().writes.load(Ordering::SeqCst), 2);
                server.cancel().await.unwrap();
            }
        }
    }).await.unwrap();
}

#[derive(Serialize, Deserialize)]
struct RequiredPatch {
    value: u8,
}
impl Model for RequiredPatch {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Single::<u8>::fields()
    }
}
impl SettingsModel for RequiredPatch {
    type Patch = Self;
}

#[derive(Serialize, Deserialize)]
struct DifferentDomain {
    value: u8,
}
impl Model for DifferentDomain {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Single::<u8>::fields()
    }
}
impl SettingsModel for DifferentDomain {
    type Patch = Partial<u16>;
}

#[test]
fn typed_definitions_reject_optional_values_required_patches_and_mismatched_domains() {
    assert_eq!(
        TypedSettings::<Single<Option<u8>>>::new()
            .unwrap_err()
            .path(),
        Some("/value")
    );
    assert_eq!(
        TypedSettings::<RequiredPatch>::new().unwrap_err().path(),
        Some("/value")
    );
    assert_eq!(
        TypedSettings::<DifferentDomain>::new().unwrap_err().path(),
        Some("/value")
    );
    assert!(
        TypedSettings::<Single<Vec<String>>>::new().is_err(),
        "form arrays cannot become native settings"
    );
    assert!(
        TypedSettings::<Values>::new()
            .unwrap()
            .with_tool_names("same", "same")
            .is_err()
    );
    assert!(
        TypedSettings::<Values>::new()
            .unwrap()
            .with_layout(vec![SettingsGroup::Group {
                title: "Display".into(),
                items: vec![SettingsLayoutItem::Property {
                    property: "unknown".into()
                }]
            }])
            .is_err()
    );
}

enum Registry<S> {
    Router(ToolRouter<S>),
    Custom(BTreeMap<String, ToolRoute<S>>),
}
struct Server {
    registry: Registry<Server>,
    info: ServerConfig,
    values: Mutex<Values>,
    reads: AtomicUsize,
    writes: AtomicUsize,
}
fn context_is_original(context: &RequestContext<RoleServer>) {
    assert_eq!(
        context.openai_client().unwrap().session_id(),
        Some("typed-settings")
    );
    assert_eq!(
        context.meta.get("test/keep"),
        Some(&json!({"trace":"original context"}))
    );
    assert!(!context.ct.is_cancelled());
}
fn read<'a>(
    server: &'a Server,
    context: RequestContext<RoleServer>,
) -> ExtensionFuture<'a, Values> {
    Box::pin(async move {
        tokio::task::yield_now().await;
        context_is_original(&context);
        server.reads.fetch_add(1, Ordering::SeqCst);
        Ok(server.values.lock().await.clone())
    })
}
fn update<'a>(
    server: &'a Server,
    patch: Patch,
    context: RequestContext<RoleServer>,
) -> ExtensionFuture<'a, Values> {
    Box::pin(async move {
        tokio::task::yield_now().await;
        context_is_original(&context);
        let mut values = server.values.lock().await;
        server.writes.fetch_add(1, Ordering::SeqCst);
        if let Some(enabled) = patch.enabled {
            values.enabled = enabled;
        }
        if let Some(count) = patch.count {
            values.count = count;
        }
        if let Some(label) = patch.label {
            values.label = label;
        }
        Ok(values.clone())
    })
}
impl Server {
    fn new(custom: bool) -> Self {
        let settings = TypedSettings::<Values>::new()
            .unwrap()
            .with_tool_names("preferences.read", "preferences.update")
            .unwrap()
            .with_layout(vec![SettingsGroup::Group {
                title: "Display".into(),
                items: vec![SettingsLayoutItem::Property {
                    property: "display/name~".into(),
                }],
            }])
            .unwrap();
        let mut info = ServerConfig::default();
        info.server_info.name = "typed-settings-test".into();
        let registry = if custom {
            let (routes, capability) = settings.into_tools_with_state(read, update).into_parts();
            capability.apply_capabilities(&mut info.capabilities);
            Registry::Custom(
                routes
                    .into_iter()
                    .map(|route| (route.name().to_owned(), route))
                    .collect(),
            )
        } else {
            let mut router = ToolRouter::new();
            let mut extensions = OpenAIExtensions::new();
            extensions
                .register_typed_settings_with_state(&mut router, settings, read, update)
                .unwrap();
            extensions.apply_capabilities(&mut info.capabilities);
            Registry::Router(router)
        };
        Self {
            registry,
            info,
            values: Mutex::new(initial()),
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
            Registry::Custom(routes) => routes.values().map(|route| route.attr.clone()).collect(),
        };
        Ok(ListToolsResult::with_all_items(tools))
    }
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let context = ToolCallContext::new(self, params, context);
        match &self.registry {
            Registry::Router(router) => router.call(context).await,
            Registry::Custom(routes) => (routes.get(context.name()).unwrap().call)(context).await,
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
        ("preferences.read", json!({}), None),
        ("preferences.update", json!({"set":{"enabled":false}}), None),
        ("preferences.update", json!({"set":{"count":0}}), None),
        (
            "preferences.update",
            json!({"set":{"display/name~":""}}),
            None,
        ),
        (
            "preferences.update",
            json!({"set":{"count":null}}),
            Some(-32602),
        ),
        ("preferences.update", json!({"set":{}}), Some(-32602)),
        (
            "preferences.update",
            json!({"set":{"unknown":true}}),
            Some(-32602),
        ),
        (
            "preferences.update",
            json!({"set":{"count":256}}),
            Some(-32602),
        ),
        (
            "preferences.update",
            json!({"set":{"count":1.5}}),
            Some(-32602),
        ),
        (
            "preferences.update",
            json!({"set":{"count":"0"}}),
            Some(-32602),
        ),
        (
            "preferences.update",
            json!({"set":{"count":0},"extra":true}),
            Some(-32602),
        ),
        ("preferences.read", json!({"extra":true}), Some(-32602)),
        (
            "preferences.update",
            json!({"set":{"count":13}}),
            Some(-32603),
        ),
        ("preferences.read", json!({}), Some(-32603)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut meta = wire::meta();
        meta["openai/session"] = json!("typed-settings");
        let response = rpc
            .request(
                3 + index as u64,
                "tools/call",
                json!({"name":name,"arguments":arguments,"_meta":meta}),
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
        server.service().reads.load(Ordering::SeqCst),
        2,
        "invalid read input must not invoke callbacks"
    );
    assert_eq!(
        server.service().writes.load(Ordering::SeqCst),
        4,
        "invalid patches must not invoke writes"
    );
    assert_eq!(
        *server.service().values.lock().await,
        Values {
            enabled: false,
            count: 13,
            label: String::new()
        },
        "output errors cannot undo persisted consumer state"
    );
    server.cancel().await.unwrap();
    observations
}

#[tokio::test]
async fn typed_callbacks_preserve_partial_updates_and_validate_wire_results_in_both_registries() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let router = observe(false).await;
        let custom = observe(true).await;
        assert_eq!(custom, router);
        assert_eq!(
            custom[0]["result"]["capabilities"]["extensions"]["openai/settings"],
            json!({"readTool":"preferences.read","updateTool":"preferences.update"})
        );
        assert_eq!(
            custom[3]["result"]["structuredContent"]["values"],
            json!({"enabled":false,"count":4,"display/name~":"retained"})
        );
        assert_eq!(
            custom[5]["result"]["structuredContent"]["values"],
            json!({"enabled":false,"count":0,"display/name~":""})
        );
        assert!(
            custom[14]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("/count")
        );
    })
    .await
    .unwrap();
}

#[test]
fn owned_typed_callbacks_use_the_same_declared_routes() {
    let tools = TypedSettings::<Values>::new()
        .unwrap()
        .into_tools::<(), _, _, _, _>(
            |_| async { Ok(initial()) },
            |patch, _| async move {
                Ok(Values {
                    enabled: patch.enabled.unwrap_or(true),
                    count: patch.count.unwrap_or(4),
                    label: patch.label.unwrap_or_else(|| "retained".into()),
                })
            },
        );
    assert_eq!(tools.read().name(), "settings.read");
    assert_eq!(
        tools.update().attr.input_schema["properties"]["set"]["minProperties"],
        1
    );
}

#[test]
fn typed_installer_preflights_the_whole_pair_before_advertising() {
    let mut router: ToolRouter<()> = ToolRouter::new();
    router.disable_route("settings.update");
    let mut extensions = OpenAIExtensions::new();
    let result = extensions.register_typed_settings(
        &mut router,
        TypedSettings::<Values>::new().unwrap(),
        |_| async { Ok(initial()) },
        |_, _| async { Ok(initial()) },
    );
    assert!(matches!(result,Err(Error::ToolCollision(ref name)) if name == "settings.update"));
    assert!(
        router.map.is_empty(),
        "failed registration cannot leave its read route installed"
    );
    let mut capabilities = Default::default();
    extensions.apply_capabilities(&mut capabilities);
    assert!(capabilities.tools.is_none());
    assert!(capabilities.extensions.is_none());
    assert!(
        router.enable_route("settings.update"),
        "the original reserved name must survive"
    );
    extensions
        .register_typed_settings(
            &mut router,
            TypedSettings::<Values>::new().unwrap(),
            |_| async { Ok(initial()) },
            |_, _| async { Ok(initial()) },
        )
        .unwrap();
    assert!(router.has_route("settings.read") && router.has_route("settings.update"));
    extensions.apply_capabilities(&mut capabilities);
    assert_eq!(
        capabilities.extensions.unwrap()["openai/settings"],
        object(json!({"readTool":"settings.read","updateTool":"settings.update"}))
    );
}

#[derive(Serialize, Deserialize)]
struct ConversionValues {
    count: u8,
}
#[derive(Debug, Serialize, Deserialize)]
struct ConversionPatch {
    count: Option<i8>,
}
impl Model for ConversionValues {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![ModelField::new::<u8>("count", "Count")?])
    }
}
impl Model for ConversionPatch {
    fn fields() -> Result<Vec<ModelField>, Error> {
        // Deliberately inconsistent handwritten metadata: the final Serde decode
        // must still reject values that cannot fit the actual Patch Rust type.
        Ok(vec![ModelField::new::<Option<u8>>("count", "Count")?])
    }
}
impl SettingsModel for ConversionValues {
    type Patch = ConversionPatch;
}
struct ConversionServer {
    router: ToolRouter<Self>,
    writes: AtomicUsize,
}
impl ConversionServer {
    fn new() -> Self {
        let tools = TypedSettings::<ConversionValues>::new()
            .unwrap()
            .into_tools_with_state(
                |_: &Self, _| Box::pin(async { Ok(ConversionValues { count: 1 }) }),
                |server: &Self, patch, _| {
                    Box::pin(async move {
                        tokio::task::yield_now().await;
                        server.writes.fetch_add(1, Ordering::SeqCst);
                        Ok(ConversionValues {
                            count: u8::try_from(patch.count.unwrap()).unwrap(),
                        })
                    })
                },
            );
        let (routes, _) = tools.into_parts();
        let mut router = ToolRouter::new();
        for route in routes {
            router.add_route(route);
        }
        Self {
            router,
            writes: AtomicUsize::new(0),
        }
    }
}
impl ServerHandler for ConversionServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.capabilities.tools = Some(Default::default());
        info
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

#[tokio::test]
async fn inconsistent_handwritten_patch_metadata_cannot_bypass_serde_conversion_before_writes() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let settings = TypedSettings::<ConversionValues>::new().unwrap();
        assert_eq!(
            settings
                .validate_patch(&object(json!({"count":128})))
                .unwrap_err()
                .path(),
            Some("")
        );
        assert_eq!(
            settings
                .decode_patch(object(json!({"count":128})))
                .unwrap_err()
                .path(),
            Some("")
        );
        let (server_io, client_io) = tokio::io::duplex(65536);
        let task = tokio::spawn(ConversionServer::new().serve(server_io));
        let mut rpc = wire::Rpc::new(client_io);
        rpc.modern(1, "server/discover", json!({})).await;
        let server = task.await.unwrap().unwrap();
        let bad = rpc
            .modern(
                2,
                "tools/call",
                json!({"name":"settings.update","arguments":{"set":{"count":128}}}),
            )
            .await;
        assert_eq!(bad["error"]["code"], -32602);
        assert_eq!(server.service().writes.load(Ordering::SeqCst), 0);
        let good = rpc
            .modern(
                3,
                "tools/call",
                json!({"name":"settings.update","arguments":{"set":{"count":127}}}),
            )
            .await;
        assert_eq!(
            good["result"]["structuredContent"],
            json!({"values":{"count":127}})
        );
        assert_eq!(server.service().writes.load(Ordering::SeqCst), 1);
        server.cancel().await.unwrap();
    })
    .await
    .unwrap();
}
