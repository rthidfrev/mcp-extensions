use openai_mcp_extensions::{
    Error,
    forms::{
        self, FormResult, OpenAIForm,
        mrtr::{FormInput, FormInputRequired, FormService, read_input},
    },
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, CustomNotification,
        ListToolsResult, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, ResourceContents, ServerConfig, Tool,
    },
    service::{NotificationContext, RequestContext},
};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

// The native probe used this valid generated PNG; no private capture path or
// account metadata enters this fixture. Its contract is the spec's MCP.Icon.
const BLUE_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAIAAAD8GO2jAAAAKklEQVR4nGPQCLhEU8QwasGoBaMWjFowasGoBaMWjFowasGoBaMWDBULALjvKEy4pBJwAAAAAElFTkSuQmCC";
fn form() -> OpenAIForm {
    OpenAIForm::from_json(json!({"type":"object","properties":{"selection":{"type":"string","title":"Color","oneOf":[{"const":"blue","title":"Blue square","x-openai-thumbnail":{"src":BLUE_PNG,"mimeType":"image/png"}}]}},"required":["selection"]})).unwrap()
}
#[path = "support/wire.rs"]
mod wire;
use wire::{Rpc, meta};

struct Inner {
    form: Arc<OpenAIForm>,
    notifications: Arc<AtomicUsize>,
}
impl ServerHandler for Inner {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.server_info.name = "inner-service".into();
        info.capabilities.resources = Some(Default::default());
        info.capabilities.extensions = Some(
            [(
                "example/keep".into(),
                json!({"value":true}).as_object().unwrap().clone(),
            )]
            .into(),
        );
        info
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
        Ok(ListToolsResult::with_all_items(
            ["form", "plain"]
                .into_iter()
                .map(|name| {
                    Tool::new(
                        name,
                        "Existing tool",
                        json!({"type":"object"}).as_object().unwrap().clone(),
                    )
                })
                .collect(),
        ))
    }
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if params.name == "form" {
            let result = forms::elicit_input(&context, "Choose a color", &self.form)
                .await
                .map_err(ErrorData::from)?;
            return Ok(CallToolResult::structured(json!(result)).into());
        }
        assert_eq!(params.name, "plain");
        Ok(CallToolResult::structured(json!({"delegated":true})).into())
    }
    async fn read_resource(
        &self,
        _: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        Ok(
            ReadResourceResult::new(vec![ResourceContents::text("unchanged", "example://data")])
                .into(),
        )
    }
    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _: NotificationContext<RoleServer>,
    ) {
        assert_eq!(notification.method, "example/notice");
        self.notifications.fetch_add(1, Ordering::SeqCst);
    }
}
#[derive(Default)]
struct Effects {
    calls: AtomicUsize,
    writes: AtomicUsize,
    next: AtomicUsize,
    pending: Mutex<BTreeMap<String, Option<serde_json::Map<String, Value>>>>,
    ids: Mutex<Vec<Value>>,
}
fn service(effects: Arc<Effects>, notifications: Arc<AtomicUsize>) -> FormService<Inner> {
    let form = Arc::new(form());
    let mut service = FormService::new(Inner {
        form: form.clone(),
        notifications,
    });
    service
        .register("form", move |params, context| {
            let form = form.clone();
            let effects = effects.clone();
            async move {
                effects.calls.fetch_add(1, Ordering::SeqCst);
                effects.ids.lock().unwrap().push(json!(context.id));
                assert_eq!(
                    context.meta.0["test/keep"],
                    json!({"trace":"original context"})
                );
                match read_input(&params, "answer", &form).map_err(ErrorData::from)? {
                    FormInput::Initial => {
                        let state =
                            format!("state-{}", effects.next.fetch_add(1, Ordering::SeqCst));
                        effects
                            .pending
                            .lock()
                            .unwrap()
                            .insert(state.clone(), params.arguments.clone());
                        Ok(
                            FormInputRequired::new(state, "answer", "Choose a color", &form)
                                .map_err(ErrorData::from)?
                                .with_meta(
                                    serde_json::from_value(
                                        json!({"example/result":{"value":true}}),
                                    )
                                    .unwrap(),
                                )
                                .into(),
                        )
                    }
                    FormInput::Response {
                        request_state,
                        result,
                    } => {
                        // This fixture owns and consumes state. The SDK's decoder
                        // deliberately neither authenticates nor persists it.
                        let mut pending = effects.pending.lock().unwrap();
                        let original = pending.get(&request_state).ok_or_else(|| {
                            ErrorData::invalid_params("unknown or consumed state", None)
                        })?;
                        if original != &params.arguments {
                            return Err(ErrorData::invalid_params("changed arguments", None));
                        }
                        pending.remove(&request_state);
                        if matches!(result, FormResult::Accept { .. }) {
                            effects.writes.fetch_add(1, Ordering::SeqCst);
                        }
                        let mut complete = CallToolResult::structured(json!(result));
                        complete.result_type = None;
                        complete.meta = Some(
                            serde_json::from_value(json!({"example/result":{"value":true}}))
                                .unwrap(),
                        );
                        Ok(complete.into())
                    }
                }
            }
        })
        .unwrap();
    assert!(matches!(
        service.register("form", |_, _| async { unreachable!() }),
        Err(Error::ToolCollision(_))
    ));
    service
}
fn continuation(state: &Value, response: Value) -> Value {
    json!({"name":"form","arguments":{"run":"original"},"requestState":state,"inputResponses":{"answer":response}})
}

#[tokio::test]
async fn modern_forms_preserve_wire_and_validate_before_effects() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let effects = Arc::new(Effects::default());
        let notifications = Arc::new(AtomicUsize::new(0));
        let (server_io, client_io) = tokio::io::duplex(65536);
        let server = tokio::spawn(service(effects.clone(), notifications.clone()).serve(server_io));
        let mut rpc = Rpc::new(client_io);
        let discovery = rpc.modern(1, "server/discover", json!({})).await;
        assert_eq!(discovery["result"]["capabilities"]["extensions"]["example/keep"], json!({"value":true}));
        let server = server.await.unwrap().unwrap();
        let tools = rpc.modern(2, "tools/list", json!({})).await;
        assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 2);
        let plain = rpc.modern(3, "tools/call", json!({"name":"plain"})).await;
        assert_eq!(plain["result"]["structuredContent"], json!({"delegated":true}));
        let read = rpc.modern(4, "resources/read", json!({"uri":"example://data"})).await;
        assert_eq!(read["result"]["contents"][0]["text"], "unchanged");
        rpc.send(json!({"jsonrpc":"2.0","method":"example/notice"})).await;

        let initial = rpc.modern(5, "tools/call", json!({"name":"form","arguments":{"run":"original"}})).await;
        assert_eq!(initial["result"]["resultType"], "input_required");
        assert_eq!(initial["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "inner-service");
        assert_eq!(initial["result"]["_meta"]["example/result"], json!({"value":true}));
        assert_eq!(initial["result"]["inputRequests"]["answer"], json!({"method":"openai/elicitation/create","params":{"mode":"form","message":"Choose a color","requestedSchema":form().as_json()}}));
        assert_eq!(effects.writes.load(Ordering::SeqCst), 0);
        let state = initial["result"]["requestState"].clone();
        let mut id = 6;
        for bad in [json!({"action":"accept","content":{}}), json!({"action":"accept","content":{"selection":"red"}}), json!({"result":{"action":"accept","content":{"selection":"blue"}}})] {
            let result = rpc.modern(id, "tools/call", continuation(&state, bad)).await;
            assert!(result.get("error").is_some());
            assert_eq!(effects.writes.load(Ordering::SeqCst), 0);
            id += 1;
        }
        for bad in [json!({"name":"form","arguments":{"run":"original"},"requestState":state}), json!({"name":"form","arguments":{"run":"original"},"inputResponses":{"answer":{"action":"accept","content":{"selection":"blue"}}}})] {
            assert!(rpc.modern(id, "tools/call", bad).await.get("error").is_some());
            assert_eq!(effects.writes.load(Ordering::SeqCst), 0);
            id += 1;
        }
        let answer = json!({"action":"accept","content":{"selection":"blue"}});
        let mut changed = continuation(&state, answer.clone());
        changed["arguments"]["run"] = json!("different");
        assert!(rpc.modern(id, "tools/call", changed).await.get("error").is_some());
        assert_eq!(effects.writes.load(Ordering::SeqCst), 0);
        id += 1;
        let complete = rpc.modern(id, "tools/call", continuation(&state, answer.clone())).await;
        assert_eq!(complete["result"]["resultType"], "complete");
        assert_eq!(complete["result"]["_meta"], initial["result"]["_meta"]);
        assert_eq!(complete["result"]["structuredContent"], answer);
        assert_eq!(effects.writes.load(Ordering::SeqCst), 1);
        id += 1;
        assert!(rpc.modern(id, "tools/call", continuation(&state, answer)).await.get("error").is_some());
        assert_eq!(effects.writes.load(Ordering::SeqCst), 1);
        id += 1;
        for action in ["decline", "cancel"] {
            let initial = rpc.modern(id, "tools/call", json!({"name":"form","arguments":{"run":"original"}})).await;
            id += 1;
            let done = rpc.modern(id, "tools/call", continuation(&initial["result"]["requestState"], json!({"action":action}))).await;
            assert_eq!(done["result"]["resultType"], "complete");
            assert_eq!(done["result"]["structuredContent"], json!({"action":action}));
            assert_eq!(effects.writes.load(Ordering::SeqCst), 1);
            id += 1;
        }
        assert!(rpc.legacy_requests.is_empty());
        assert!(effects.ids.lock().unwrap().contains(&json!(5)));
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        server.cancel().await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn modern_request_gates_run_before_the_callback() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let effects = Arc::new(Effects::default());
        let (server_io, client_io) = tokio::io::duplex(65536);
        let server = tokio::spawn(service(effects.clone(), Default::default()).serve(server_io));
        let mut rpc = Rpc::new(client_io);
        rpc.modern(1, "server/discover", json!({})).await;
        let server = server.await.unwrap().unwrap();
        let mut without_capability = meta();
        without_capability["io.modelcontextprotocol/clientCapabilities"] = json!({});
        let rejected = rpc
            .request(
                2,
                "tools/call",
                json!({"name":"form","_meta":without_capability}),
            )
            .await;
        assert_eq!(rejected["error"]["code"], -32021);
        assert_eq!(
            rejected["error"]["data"]["requiredCapabilities"]["extensions"]["openai/elicitation"],
            json!({"form":{}})
        );
        let mut id = 3;
        for key in [
            "io.modelcontextprotocol/protocolVersion",
            "io.modelcontextprotocol/clientCapabilities",
        ] {
            let mut missing = meta();
            missing.as_object_mut().unwrap().remove(key);
            let rejected = rpc
                .request(id, "tools/call", json!({"name":"form","_meta":missing}))
                .await;
            assert_eq!(
                rejected["error"]["code"], -32602,
                "missing {key}: {rejected}"
            );
            id += 1;
        }
        let mut unsupported = meta();
        unsupported["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
        assert_eq!(
            rpc.request(id, "tools/call", json!({"name":"form","_meta":unsupported}))
                .await["error"]["code"],
            -32022
        );
        assert_eq!(effects.calls.load(Ordering::SeqCst), 0);
        assert_eq!(effects.writes.load(Ordering::SeqCst), 0);
        // MCP's metadata table requires version and capabilities; clientInfo
        // is recommended, optional and never an authentication identity.
        id += 1;
        let mut without_info = meta();
        without_info
            .as_object_mut()
            .unwrap()
            .remove("io.modelcontextprotocol/clientInfo");
        let accepted = rpc
            .request(
                id,
                "tools/call",
                json!({"name":"form","_meta":without_info}),
            )
            .await;
        assert_eq!(accepted["result"]["resultType"], "input_required");
        assert_eq!(effects.calls.load(Ordering::SeqCst), 1);
        assert_eq!(effects.writes.load(Ordering::SeqCst), 0);
        server.cancel().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn adapter_keeps_the_legacy_direct_form_flow() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let effects = Arc::new(Effects::default());
        let (server_io, client_io) = tokio::io::duplex(65536);
        let server = tokio::spawn(service(effects.clone(), Default::default()).serve(server_io));
        let mut rpc = Rpc::new(client_io);
        rpc.request(1,"initialize",json!({"protocolVersion":"2025-11-25","capabilities":{"extensions":{"openai/elicitation":{"form":{}}}},"clientInfo":{"name":"legacy-host","version":"1"}})).await;
        rpc.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
        let server = server.await.unwrap().unwrap();
        rpc.legacy_answer = Some(json!({"action":"accept","content":{"selection":"blue"}}));
        let complete = rpc.request(2,"tools/call",json!({"name":"form"})).await;
        assert_eq!(complete["result"]["structuredContent"], rpc.legacy_answer.clone().unwrap());
        assert_eq!(rpc.legacy_requests.len(), 1);
        assert_eq!(rpc.legacy_requests[0]["params"]["requestedSchema"], form().as_json().clone());
        assert_eq!(effects.calls.load(Ordering::SeqCst), 0);
        let invalid = rpc.request(3,"tools/call",continuation(&json!("old"),json!({"action":"cancel"}))).await;
        assert!(invalid.get("error").is_some());
        assert_eq!(rpc.legacy_requests.len(), 1);
        server.cancel().await.unwrap();
    }).await.unwrap();
}

#[test]
fn multiple_form_keys_and_result_metadata_are_lossless() {
    let form = form();
    let mut required =
        FormInputRequired::new("opaque state", "first", "First choice", &form).unwrap();
    let unchanged = required.as_json();
    assert!(required.add_form("first", "Replacement", &form).is_err());
    assert_eq!(required.as_json(), unchanged);
    assert!(required.add_form(" ", "Invalid", &form).is_err());
    required.add_form("second", "Second choice", &form).unwrap();
    let required =
        required.with_meta(serde_json::from_value(json!({"example/keep":{"value":true}})).unwrap());
    let value = required.as_json();
    assert_eq!(value["_meta"], json!({"example/keep":{"value":true}}));
    assert_eq!(
        value["inputRequests"]["second"]["params"]["requestedSchema"],
        *form.as_json()
    );
    let params: CallToolRequestParams = serde_json::from_value(json!({"name":"form","requestState":"opaque state","inputResponses":{"first":{"action":"accept","content":{"selection":"blue"}},"second":{"action":"cancel"}}})).unwrap();
    assert!(matches!(
        read_input(&params, "first", &form).unwrap(),
        FormInput::Response {
            result: FormResult::Accept { .. },
            ..
        }
    ));
    assert!(matches!(
        read_input(&params, "second", &form).unwrap(),
        FormInput::Response {
            result: FormResult::Cancel,
            ..
        }
    ));
    assert!(read_input(&params, "missing", &form).is_err());
}
