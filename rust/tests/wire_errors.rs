use openai_mcp_extensions::{
    Error,
    forms::{self, FormField, OpenAIForm, mrtr::FormInputRequired},
    mentions::{self, MentionSearchResult},
    resources::{requested_representation, resource_tool_call_metadata},
    settings::{SettingsGroup, SettingsLayoutItem, SettingsRegistration},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorCode, JsonObject,
        ListToolsResult, ProtocolVersion, ServerConfig,
    },
    service::{RequestContext, ServiceError},
    transport::DynamicTransportError,
};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[path = "support/wire.rs"]
mod wire;

#[test]
fn transport_closure_is_not_reported_as_invalid_parameters() {
    let error = ErrorData::from(Error::Transport(ServiceError::TransportClosed));
    assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
}

#[test]
fn materially_different_failures_keep_their_protocol_categories() {
    let malformed = serde_json::from_value::<bool>(json!("synthetic-private-input")).unwrap_err();
    let invalid_definition = OpenAIForm::from_json(json!({})).unwrap_err();
    let invalid_settings = SettingsRegistration::new(json!({})).unwrap_err();
    for (error, expected) in [
        (
            Error::Validation("invalid input".into()),
            ErrorCode::INVALID_PARAMS,
        ),
        (Error::Json(malformed), ErrorCode::INVALID_PARAMS),
        (invalid_definition, ErrorCode::INTERNAL_ERROR),
        (invalid_settings, ErrorCode::INTERNAL_ERROR),
        (
            Error::ToolCollision("duplicate".into()),
            ErrorCode::INTERNAL_ERROR,
        ),
        (Error::UnsupportedProtocol, ErrorCode::INVALID_REQUEST),
        (
            Error::UnsupportedClient,
            ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
        ),
    ] {
        assert_eq!(ErrorData::from(error).code, expected);
    }
    let capability = serde_json::to_value(ErrorData::from(Error::UnsupportedClient)).unwrap();
    assert_eq!(
        capability["data"]["requiredCapabilities"],
        json!({
            "extensions":{"openai/elicitation":{"form":{}}}
        })
    );
    let form = OpenAIForm::from_json(json!({"type":"object","properties":{}})).unwrap();
    for (key, message) in [(" ", "Message"), ("answer", " ")] {
        let error = FormInputRequired::new("state", key, message, &form).unwrap_err();
        assert_eq!(ErrorData::from(error).code, ErrorCode::INTERNAL_ERROR);
    }
    let mut request = FormInputRequired::new("state", "answer", "Message", &form).unwrap();
    let error = request
        .add_form("answer", "Another message", &form)
        .unwrap_err();
    assert_eq!(ErrorData::from(error).code, ErrorCode::INTERNAL_ERROR);
}

#[test]
fn local_service_failures_are_internal_without_echoing_transport_details() {
    let hidden = "synthetic-private-transport-detail";
    for source in [
        ServiceError::TransportClosed,
        ServiceError::UnexpectedResponse,
        ServiceError::Cancelled {
            reason: Some(hidden.into()),
        },
        ServiceError::Timeout {
            timeout: Duration::from_millis(20),
        },
        ServiceError::SubscriptionLagged { capacity: 1 },
        ServiceError::InputRequiredRoundsExceeded { max_rounds: 2 },
        ServiceError::TransportSend(DynamicTransportError::from_parts(
            hidden,
            std::any::TypeId::of::<()>(),
            Box::new(std::io::Error::other(hidden)),
        )),
    ] {
        let error = ErrorData::from(Error::Transport(source));
        assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
        assert!(!serde_json::to_string(&error).unwrap().contains(hidden));
    }
    let peer = ErrorData::new(
        ErrorCode(-32099),
        "Peer rejected the request",
        Some(json!({"path":"/peer","example/diagnostic":{"keep":true}})),
    );
    assert_eq!(
        ErrorData::from(Error::Transport(ServiceError::McpError(peer.clone()))),
        peer
    );
}

#[test]
fn structured_paths_survive_context_and_conversion_without_submitted_values() {
    let path = "/a~1b~0c";
    for (error, expected) in [
        (
            Error::ModelValidation {
                path: path.into(),
                message: "invalid field".into(),
            },
            ErrorCode::INVALID_PARAMS,
        ),
        (
            Error::Configuration {
                source: Box::new(Error::ModelValidation {
                    path: path.into(),
                    message: "invalid definition".into(),
                }),
            },
            ErrorCode::INTERNAL_ERROR,
        ),
        (
            Error::InvalidResponse {
                source: Box::new(Error::ModelValidation {
                    path: path.into(),
                    message: "invalid answer".into(),
                }),
            },
            ErrorCode::INTERNAL_ERROR,
        ),
    ] {
        assert_eq!(error.path(), Some(path));
        let error = ErrorData::from(error);
        assert_eq!(error.code, expected);
        assert_eq!(error.data, Some(json!({"path":path})));
    }

    let hidden = "synthetic-private-submitted-value";
    let source = serde_json::from_value::<bool>(json!(hidden)).unwrap_err();
    let error = Error::ModelConversion {
        path: path.into(),
        source,
    };
    assert!(std::error::Error::source(&error).is_some());
    assert!(!error.to_string().contains(hidden));
    let error = ErrorData::from(error);
    assert_eq!(error.data, Some(json!({"path":path})));
    assert!(!serde_json::to_string(&error).unwrap().contains(hidden));

    let settings = SettingsRegistration::new(json!({"type":"object","properties":{
        "a/b~c":{"type":"string","title":"Value","enum":["allowed"]}
    }}))
    .unwrap();
    let error = settings
        .validate_patch(json!({"a/b~c":hidden}).as_object().unwrap())
        .unwrap_err();
    assert_eq!(error.path(), Some(path));
    assert!(
        !serde_json::to_string(&ErrorData::from(error))
            .unwrap()
            .contains(hidden)
    );

    let definition = FormField::from_json(json!({"type":"string","minLength":hidden})).unwrap_err();
    assert_eq!(definition.path(), Some("/minLength"));
    let definition = ErrorData::from(definition);
    assert_eq!(definition.code, ErrorCode::INTERNAL_ERROR);
    assert!(!serde_json::to_string(&definition).unwrap().contains(hidden));

    // Host metadata remains an input failure, even though its JSON source may
    // contain the rejected representation or path value.
    for meta in [
        json!({"openai/resource":{"representation":hidden}}),
        json!({"openai/resource":{"path":[hidden]}}),
    ] {
        let meta = serde_json::from_value(meta).unwrap();
        let error = requested_representation(&meta)
            .err()
            .or_else(|| resource_tool_call_metadata(&meta).err())
            .unwrap();
        let error = ErrorData::from(error);
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert!(!serde_json::to_string(&error).unwrap().contains(hidden));
    }
    let params = serde_json::from_value(json!({
        "name":"form","inputResponses":{"answer":{"action":hidden}},"requestState":"opaque"
    }))
    .unwrap();
    let form = OpenAIForm::from_json(json!({"type":"object","properties":{}})).unwrap();
    let error = forms::mrtr::read_input(&params, "answer", &form).unwrap_err();
    let error = ErrorData::from(error);
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    assert!(!serde_json::to_string(&error).unwrap().contains(hidden));
}

fn rich_form_schema() -> Value {
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{
        "asset":{"type":"string","format":"uri","x-openai-input":{"type":"resource","options":[
            {"uri":"example://file","name":"File","title":"Reference","mimeType":"text/plain",
             "_meta":{"example/unknown":{"keep":[1,true]},"openai/preview":{"target":{
                 "type":"resource_link","uri":"example://file","name":"File","_meta":{"example/nested":{"keep":true}}
             }}}}
        ]}}
    },"required":["asset"]})
}

#[test]
fn form_envelopes_preserve_rich_schema_state_and_optional_metadata_exactly() {
    let schema = rich_form_schema();
    let form = OpenAIForm::from_json(schema.clone()).unwrap();
    let result = FormInputRequired::new("", "answer", "Choose a file", &form).unwrap();
    let expected = json!({
        "resultType":"input_required","requestState":"","inputRequests":{
            "answer":{"method":"openai/elicitation/create","params":{
                "mode":"form","message":"Choose a file","requestedSchema":schema
            }}
        }
    });
    assert_eq!(result.as_json(), expected);
    assert!(result.as_json().get("_meta").is_none());
    let meta = json!({"example/result":{"keep":[1,true]}});
    let result = result.with_meta(serde_json::from_value(meta.clone()).unwrap());
    let mut expected = expected;
    expected["_meta"] = meta;
    assert_eq!(result.as_json(), expected);
}

#[test]
fn mention_schema_matches_nonblank_presentations_and_preserves_native_metadata() {
    let route = mentions::search_tool(|_params, _context| async {
        Ok::<_, ErrorData>(MentionSearchResult::default())
    });
    let route: rmcp::handler::server::router::tool::ToolRoute<()> = route;
    let schema = serde_json::to_value(route.attr.output_schema.as_ref().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for blank in ["", " \n\t", "\u{a0}"] {
        for field in ["resourceUri", "title", "subtitle"] {
            let mut item = json!({"type":"resource","resourceUri":"example://file","title":"File","subtitle":"Detail"});
            item[field] = json!(blank);
            let value = json!({"items":[item]});
            assert!(!validator.is_valid(&value), "{field}: {value}");
            let result: MentionSearchResult = serde_json::from_value(value).unwrap();
            assert!(result.validate().is_err());
        }
        for field in ["uri", "name"] {
            let mut item = json!({"type":"resource_link","uri":"example://file","name":"File"});
            item[field] = json!(blank);
            let value = json!({"items":[item]});
            assert!(!validator.is_valid(&value));
            let result: MentionSearchResult = serde_json::from_value(value).unwrap();
            assert!(result.validate().is_err());
        }
    }
    let value = json!({"items":[
        {"type":"resource_link","uri":"example://file","name":"File","_meta":{"example/unknown":{"keep":true}}},
        {"type":"resource","resourceUri":"example://file","title":"File","icons":[{"src":"https://example.com/icon.svg","sizes":["any"],"theme":"light"}]}
    ]});
    assert!(validator.is_valid(&value));
    let result: MentionSearchResult = serde_json::from_value(value.clone()).unwrap();
    result.validate().unwrap();
    assert_eq!(serde_json::to_value(result).unwrap(), value);
    assert!(!validator.is_valid(&json!({"items":[{
        "type":"resource","resourceUri":"example://file","title":"File","icons":[{}]
    }]})));
}

struct Server {
    router: ToolRouter<Self>,
    form: OpenAIForm,
}

impl Server {
    fn new(writes: Arc<AtomicUsize>, layout: Option<Vec<SettingsGroup>>) -> Self {
        let registration = SettingsRegistration::new(settings_schema()).unwrap();
        let registration = match layout {
            Some(layout) => registration.with_layout(layout).unwrap(),
            None => registration,
        };
        let tools = registration.into_tools(
            |_| async { Ok(json!({"count":7}).as_object().unwrap().clone()) },
            move |set: JsonObject, _| {
                let writes = writes.clone();
                async move {
                    writes.fetch_add(1, Ordering::SeqCst);
                    Ok(if set["count"] == 13 {
                        json!({"count":0}).as_object().unwrap().clone()
                    } else {
                        set
                    })
                }
            },
        );
        let mut router = ToolRouter::new();
        for route in tools.into_parts().0 {
            router.add_route(route);
        }
        Self {
            router,
            form: OpenAIForm::from_json(rich_form_schema()).unwrap(),
        }
    }
}

impl ServerHandler for Server {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![
            ProtocolVersion::V_2026_07_28,
            ProtocolVersion::V_2025_11_25,
        ])
    }
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default()
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
        if params.name == "form" {
            let result = forms::elicit_input(&context, "Choose a file", &self.form)
                .await
                .map_err(ErrorData::from)?;
            return Ok(CallToolResult::structured(serde_json::to_value(result).unwrap()).into());
        }
        self.router
            .call(ToolCallContext::new(self, params, context))
            .await
    }
}

fn settings_schema() -> Value {
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","title":"Preferences","properties":{
        "count":{"type":"integer","minimum":1,"title":"Count"}
    }})
}

#[tokio::test]
async fn settings_wire_and_error_routes_preserve_callback_boundaries() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for layout in [
            None,
            Some(vec![SettingsGroup::Group {
                title: "General".into(),
                items: vec![SettingsLayoutItem::Property {
                    property: "count".into(),
                }],
            }]),
        ] {
            let writes = Arc::new(AtomicUsize::new(0));
            let (server_io, client_io) = tokio::io::duplex(65536);
            let task = tokio::spawn(Server::new(writes.clone(), layout.clone()).serve(server_io));
            let mut rpc = wire::Rpc::new(client_io);
            rpc.modern(1, "server/discover", json!({})).await;
            let server = task.await.unwrap().unwrap();
            let tools = rpc.modern(2, "tools/list", json!({})).await;
            let read = rpc
                .modern(
                    3,
                    "tools/call",
                    json!({"name":"settings.read","arguments":{}}),
                )
                .await;
            let mut expected = json!({"schema":settings_schema(),"values":{"count":7}});
            if let Some(layout) = layout {
                expected["layout"] = json!(layout);
            }
            assert_eq!(read["result"]["structuredContent"], expected);
            let read_schema = tools["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == "settings.read")
                .unwrap()["outputSchema"]
                .clone();
            assert!(
                jsonschema::validator_for(&read_schema)
                    .unwrap()
                    .is_valid(&expected)
            );

            for (id, args, expected_code) in [
                (
                    4,
                    json!({"set":{"count":0}}),
                    Some(ErrorCode::INVALID_PARAMS),
                ),
                (
                    5,
                    json!({"set":"synthetic-private-input"}),
                    Some(ErrorCode::INVALID_PARAMS),
                ),
                (
                    6,
                    json!({"set":{"count":13}}),
                    Some(ErrorCode::INTERNAL_ERROR),
                ),
                (7, json!({"set":{"count":2}}), None),
            ] {
                let response = rpc
                    .modern(
                        id,
                        "tools/call",
                        json!({"name":"settings.update","arguments":args}),
                    )
                    .await;
                match expected_code {
                    Some(code) => {
                        assert_eq!(response["error"]["code"], json!(code));
                        assert!(!response.to_string().contains("synthetic-private-input"));
                        if id != 5 {
                            assert_eq!(response["error"]["data"], json!({"path":"/count"}));
                        }
                    }
                    None => assert_eq!(
                        response["result"]["structuredContent"],
                        json!({"values":{"count":2}})
                    ),
                }
                assert_eq!(
                    writes.load(Ordering::SeqCst),
                    match id {
                        4 | 5 => 0,
                        6 => 1,
                        _ => 2,
                    }
                );
            }
            server.cancel().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn legacy_requests_share_the_lossless_envelope_and_classify_invalid_answers() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (server_io, client_io) = tokio::io::duplex(65536);
        let task = tokio::spawn(Server::new(Arc::new(AtomicUsize::new(0)), None).serve(server_io));
        let mut rpc = wire::Rpc::new(client_io);
        rpc.request(1, "initialize", json!({
            "protocolVersion":"2025-11-25","capabilities":{"extensions":{"openai/elicitation":{"form":{}}}},
            "clientInfo":{"name":"wire-host","version":"1"}
        })).await;
        rpc.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
        let server = task.await.unwrap().unwrap();
        for (id, answer) in [
            (2, json!({"action":"accept","content":{"asset":"example://file"}})),
            (3, json!({"action":"synthetic-private-answer"})),
            (4, json!({"action":"accept","content":{"asset":"example://other"}})),
        ] {
            rpc.legacy_answer = Some(answer);
            let response = rpc.request(id, "tools/call", json!({"name":"form","arguments":{}})).await;
            let mut params = rpc.legacy_requests.last().unwrap()["params"].clone();
            // rmcp adds its own progress token after serializing SDK parameters.
            // Its generated token is opaque; the SDK envelope stays exact.
            let meta = params.as_object_mut().unwrap().remove("_meta").unwrap();
            assert_eq!(meta.as_object().unwrap().len(), 1);
            assert!(meta["progressToken"].is_number() || meta["progressToken"].is_string());
            assert_eq!(params, json!({
                "mode":"form","message":"Choose a file","requestedSchema":rich_form_schema()
            }));
            if id == 2 { assert_eq!(response["result"]["structuredContent"]["action"], "accept"); }
            else {
                assert_eq!(response["error"]["code"], json!(ErrorCode::INTERNAL_ERROR));
                assert!(!response.to_string().contains("synthetic-private-answer"));
                assert!(!response.to_string().contains("example://other"));
            }
        }
        server.cancel().await.unwrap();
    }).await.unwrap();
}
