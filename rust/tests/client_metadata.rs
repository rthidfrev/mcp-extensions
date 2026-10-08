use openai_mcp_extensions::{
    Error, OpenAIRequestContextExt,
    client::{ClientMetadata, ClientMetadataReader},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::tool::ToolCallContext,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorCode, RequestMetaObject,
    },
    service::RequestContext,
};
use serde_json::{Value, json};

#[path = "support/wire.rs"]
mod wire;

fn meta(value: Value) -> RequestMetaObject {
    serde_json::from_value(value).unwrap()
}

fn assert_input_error(error: Error, path: &str) {
    assert!(matches!(error, Error::ModelValidation { .. }));
    assert_eq!(error.path(), Some(path));
    let rpc = ErrorData::from(error);
    assert_eq!(rpc.code, ErrorCode::INVALID_PARAMS);
    assert_eq!(rpc.data, Some(json!({"path": path})));
}

#[test]
fn valid_session_is_readable_even_when_strict_location_validation_fails() {
    let meta = meta(json!({
        "openai/session":"conversation-7",
        "openai/userLocation":{"longitude":"private-invalid-coordinate"},
        "example/other":{"keep":true}
    }));
    let before = meta.clone();
    let strict = ClientMetadata::from_meta(&meta).unwrap_err();
    assert_eq!(strict.path(), Some("/openai~1userLocation/longitude"));
    let fields = ClientMetadataReader::new(&meta);
    assert_eq!(fields.session_id().unwrap(), Some("conversation-7"));
    let location = fields.location().unwrap_err();
    assert!(!location.to_string().contains("private-invalid-coordinate"));
    assert_input_error(location, "/openai~1userLocation/longitude");
    assert_eq!(meta, before);
}

#[test]
fn strict_and_independent_reads_preserve_text_and_location_normalization() {
    let meta = meta(json!({
        "openai/session":" session ","openai/subject":"subject-4",
        "openai/organization":"organization-2","openai/locale":" fr-FR ",
        "webplus/i18n":false,"openai/userAgent":"Synthetic client",
        "openai/userLocation":{
            "city":" Paris ","region":"IDF","country":"FR","timezone":"Europe/Paris",
            "longitude":" 2.35 ","latitude":48.86,"unknown":{"keep":true}
        }
    }));
    let before = meta.clone();
    let strict = ClientMetadata::from_meta(&meta).unwrap();
    let fields = ClientMetadataReader::new(&meta);
    assert_eq!(fields.session_id().unwrap(), Some(" session "));
    assert_eq!(fields.user_id().unwrap(), Some("subject-4"));
    assert_eq!(fields.organization_id().unwrap(), Some("organization-2"));
    assert_eq!(fields.locale().unwrap(), Some(" fr-FR "));
    assert_eq!(fields.user_agent().unwrap(), Some("Synthetic client"));
    assert_eq!(strict.session_id(), fields.session_id().unwrap());
    assert_eq!(strict.user_id(), fields.user_id().unwrap());
    assert_eq!(strict.organization_id(), fields.organization_id().unwrap());
    assert_eq!(strict.locale(), fields.locale().unwrap());
    assert_eq!(strict.user_agent(), fields.user_agent().unwrap());
    let location = fields.location().unwrap().unwrap();
    assert_eq!(strict.location(), Some(&location));
    assert_eq!(location.city(), Some(" Paris "));
    assert_eq!(location.region(), Some("IDF"));
    assert_eq!(location.country(), Some("FR"));
    assert_eq!(location.timezone(), Some("Europe/Paris"));
    assert_eq!(location.longitude(), Some(2.35));
    assert_eq!(location.latitude(), Some(48.86));
    assert_eq!(meta, before);
}

#[test]
fn absence_null_and_wrong_types_remain_distinct_for_each_reader() {
    let empty = RequestMetaObject::default();
    let fields = ClientMetadataReader::new(&empty);
    for result in [
        fields.session_id(),
        fields.user_id(),
        fields.organization_id(),
        fields.locale(),
        fields.user_agent(),
    ] {
        assert_eq!(result.unwrap(), None);
    }
    assert_eq!(fields.location().unwrap(), None);
    assert_eq!(
        ClientMetadata::from_meta(&empty).unwrap(),
        ClientMetadata::default()
    );
    for meta in [
        meta(json!({
            "openai/session":null,"openai/subject":null,"openai/organization":null,
            "openai/locale":null,"openai/userAgent":null,"openai/userLocation":null
        })),
        meta(json!({
            "openai/session":7,"openai/subject":[],"openai/organization":false,
            "openai/locale":{},"openai/userAgent":4,"openai/userLocation":false
        })),
    ] {
        let fields = ClientMetadataReader::new(&meta);
        for (result, path) in [
            (fields.session_id(), "/openai~1session"),
            (fields.user_id(), "/openai~1subject"),
            (fields.organization_id(), "/openai~1organization"),
            (fields.locale(), "/openai~1locale"),
            (fields.user_agent(), "/openai~1userAgent"),
        ] {
            assert_input_error(result.unwrap_err(), path);
        }
        assert_input_error(fields.location().unwrap_err(), "/openai~1userLocation");
    }
}

#[test]
fn modern_locale_wins_and_a_malformed_modern_locale_never_falls_back() {
    let legacy = meta(json!({"webplus/i18n":"fr"}));
    assert_eq!(
        ClientMetadataReader::new(&legacy).locale().unwrap(),
        Some("fr")
    );
    assert_eq!(
        ClientMetadata::from_meta(&legacy).unwrap().locale(),
        Some("fr")
    );
    for value in [Value::Null, json!(false)] {
        let modern = meta(json!({"openai/locale":value,"webplus/i18n":"fr"}));
        assert_input_error(
            ClientMetadataReader::new(&modern).locale().unwrap_err(),
            "/openai~1locale",
        );
        assert_eq!(
            ClientMetadata::from_meta(&modern).unwrap_err().path(),
            Some("/openai~1locale")
        );
    }
    let invalid_legacy = meta(json!({"webplus/i18n":null}));
    assert_input_error(
        ClientMetadataReader::new(&invalid_legacy)
            .locale()
            .unwrap_err(),
        "/webplus~1i18n",
    );
}

#[test]
fn location_errors_identify_the_nested_field_and_require_finite_coordinates() {
    let empty_location = meta(json!({"openai/userLocation":{}}));
    let location = ClientMetadataReader::new(&empty_location)
        .location()
        .unwrap()
        .unwrap();
    assert_eq!(location.city(), None);
    assert_eq!(location.longitude(), None);
    assert_eq!(location.latitude(), None);
    let null_city = meta(json!({"openai/userLocation":{"city":null}}));
    assert_input_error(
        ClientMetadataReader::new(&null_city)
            .location()
            .unwrap_err(),
        "/openai~1userLocation/city",
    );
    for value in [json!("NaN"), json!("inf"), json!(true), Value::Null] {
        let bad = meta(json!({"openai/userLocation":{"latitude":value}}));
        assert_input_error(
            ClientMetadataReader::new(&bad).location().unwrap_err(),
            "/openai~1userLocation/latitude",
        );
    }
}

struct MetadataServer;

impl ServerHandler for MetadataServer {
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let before = context.meta.clone();
        assert_eq!(
            context.openai_client_fields().session_id().unwrap(),
            Some("wire-session")
        );
        assert_eq!(
            context.openai_client().unwrap_err().path(),
            Some("/openai~1userLocation/longitude")
        );
        let tool_context = ToolCallContext::new(self, params, context);
        let fields = tool_context.openai_client_fields();
        let session = fields.session_id().unwrap();
        let error = fields.location().unwrap_err();
        assert_eq!(
            tool_context.openai_client().unwrap_err().path(),
            error.path()
        );
        assert_eq!(tool_context.request_context.meta, before);
        Ok(
            CallToolResult::structured(json!({"session":session,"locationErrorPath":error.path()}))
                .into(),
        )
    }
}

#[tokio::test]
async fn both_context_readers_borrow_the_original_request_metadata() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (server_io, client_io) = tokio::io::duplex(65536);
        let task = tokio::spawn(MetadataServer.serve(server_io));
        let mut rpc = wire::Rpc::new(client_io);
        rpc.modern(1, "server/discover", json!({})).await;
        let server = task.await.unwrap().unwrap();
        let mut params = json!({"name":"read_metadata","arguments":{},"_meta":wire::meta()});
        params["_meta"]["openai/session"] = json!("wire-session");
        params["_meta"]["openai/userLocation"] = json!({"longitude":"bad"});
        let response = rpc.request(2, "tools/call", params).await;
        assert_eq!(
            response["result"]["structuredContent"],
            json!({
                "session":"wire-session","locationErrorPath":"/openai~1userLocation/longitude"
            })
        );
        assert!(response.get("error").is_none());
        server.cancel().await.unwrap();
    })
    .await
    .unwrap();
}
