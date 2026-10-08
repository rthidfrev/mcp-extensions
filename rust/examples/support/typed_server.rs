//! Private application logic shared by the handwritten and derived examples.
//! This module uses their concrete models, not a generic SDK server abstraction.

use super::{Answer, Preferences, PreferencesPatch, Units};
use std::{borrow::Cow, collections::BTreeMap};

use openai_mcp_extensions::{
    Error, ExtensionFuture, OpenAIExtensions,
    forms::{
        TypedForm, TypedFormInput, TypedFormResult,
        mrtr::{self, FormInputRequired, FormToolResponse},
    },
    settings::{SettingsGroup, SettingsLayoutItem, TypedSettings},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, Service, ServiceExt,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
    },
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ClientNotification, ClientRequest,
        JsonObject, ListToolsResult, ProtocolVersion, ServerConfig, ServerResult, Tool,
    },
    service::{NotificationContext, RequestContext},
};
use serde::Serialize;
use serde_json::json;
use tokio::sync::Mutex;

const FORM_TOOL: &str = "choose_quantity";
const FORM_MESSAGE: &str = "Choose a part code (AB-123), an even quantity and measurement units";
const FORM_STATE: &str = "quantity-demo-v1";

enum Registry {
    Router(ToolRouter<Server>),
    Custom(BTreeMap<String, ToolRoute<Server>>),
}

struct Server {
    registry: Registry,
    info: ServerConfig,
    values: Mutex<Preferences>,
    form: TypedForm<Answer>,
}

fn read<'a>(
    server: &'a Server,
    context: RequestContext<RoleServer>,
) -> ExtensionFuture<'a, Preferences> {
    Box::pin(async move {
        let values = server.values.lock().await;
        check_cancelled(&context)?;
        Ok(values.clone())
    })
}

fn update<'a>(
    server: &'a Server,
    patch: PreferencesPatch,
    context: RequestContext<RoleServer>,
) -> ExtensionFuture<'a, Preferences> {
    Box::pin(async move {
        let mut values = server.values.lock().await;
        check_cancelled(&context)?;
        // The SDK validated the entire nonempty patch before invoking us.
        // This public demo changes only process-local state, with no persistence.
        if let Some(show_grid) = patch.show_grid {
            values.show_grid = show_grid;
        }
        if let Some(units) = patch.units {
            values.units = units;
        }
        if let Some(default_quantity) = patch.default_quantity {
            values.default_quantity = default_quantity;
        }
        Ok(values.clone())
    })
}

impl Server {
    fn new(use_router: bool) -> Result<Self, Error> {
        let form = TypedForm::<Answer>::new()?;
        let settings =
            TypedSettings::<Preferences>::new()?.with_layout(vec![SettingsGroup::new(
                "Display",
                [
                    SettingsLayoutItem::property("showGrid"),
                    SettingsLayoutItem::property("units"),
                    SettingsLayoutItem::property("defaultQuantity"),
                ],
            )])?;
        let ordinary = ToolRoute::new_dyn(
            Tool::new(
                FORM_TOOL,
                "Read a quantity selection without ordering or changing settings",
                json!({"type":"object","additionalProperties":false})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            |context: ToolCallContext<'_, Self>| {
                Box::pin(async move {
                    validate_form_arguments(&context.arguments)?;
                    let result = context
                        .service
                        .form
                        .elicit_input(&context.request_context, FORM_MESSAGE)
                        .await
                        .map_err(ErrorData::from)?;
                    check_cancelled(&context.request_context)?;
                    Ok(form_reply(result)?.into())
                })
            },
        );
        let mut info = ServerConfig::default();
        info.server_info.name = "typed-quantity-example".into();
        let registry = if use_router {
            let mut router = ToolRouter::new();
            router.add_route(ordinary);
            let mut extensions = OpenAIExtensions::new();
            extensions.register_typed_settings_with_state(&mut router, settings, read, update)?;
            extensions.apply_capabilities(&mut info.capabilities);
            Registry::Router(router)
        } else {
            let mut tools = BTreeMap::new();
            tools.insert(ordinary.name().to_owned(), ordinary);
            let (routes, capability) = settings.into_tools_with_state(read, update).into_parts();
            // Check the complete pair before either insert. This registry has
            // no disabled routes and no fallible operation between the inserts.
            for route in &routes {
                if tools.contains_key(route.name()) {
                    return Err(Error::ToolCollision(route.name().into()));
                }
            }
            for route in routes {
                tools.insert(route.name().to_owned(), route);
            }
            capability.apply_capabilities(&mut info.capabilities);
            Registry::Custom(tools)
        };
        Ok(Self {
            registry,
            info,
            values: Mutex::new(Preferences {
                show_grid: true,
                units: Units::Mm,
                default_quantity: 2,
            }),
            form,
        })
    }

    async fn choose_quantity(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<FormToolResponse, ErrorData> {
        validate_form_arguments(&params.arguments)?;
        check_cancelled(&context)?;
        match self
            .form
            .read_input(&params, "answer")
            .map_err(ErrorData::from)?
        {
            TypedFormInput::Initial => Ok(FormInputRequired::new(
                FORM_STATE,
                "answer",
                FORM_MESSAGE,
                self.form.raw_form(),
            )
            .map_err(ErrorData::from)?
            .into()),
            TypedFormInput::Response {
                request_state,
                result,
            } => {
                // This static marker only identifies a public, read-only demo.
                // It is not authorization. Tools with effects need caller/argument
                // binding, integrity, expiry and atomic consumption of saved state.
                if request_state != FORM_STATE {
                    return Err(ErrorData::invalid_params("unknown quantity state", None));
                }
                Ok(form_reply(result)?.into())
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Selection {
    part_code: String,
    quantity: u8,
    units: Units,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum SelectionReply {
    Accept { content: Selection },
    Cancel,
    Decline,
}

fn form_reply(result: TypedFormResult<Answer>) -> Result<CallToolResult, ErrorData> {
    let reply = match result {
        TypedFormResult::Accept { content } => SelectionReply::Accept {
            content: Selection {
                part_code: content.part_code,
                quantity: content.quantity,
                units: content.units,
                note: content.note,
            },
        },
        TypedFormResult::Cancel => SelectionReply::Cancel,
        TypedFormResult::Decline => SelectionReply::Decline,
    };
    let content = serde_json::to_value(reply)
        .map_err(|_| ErrorData::internal_error("could not serialize quantity reply", None))?;
    Ok(CallToolResult::structured(content))
}

fn check_cancelled(context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
    if context.ct.is_cancelled() {
        return Err(ErrorData::invalid_request("request cancelled", None));
    }
    Ok(())
}

fn validate_form_arguments(arguments: &Option<JsonObject>) -> Result<(), ErrorData> {
    if arguments
        .as_ref()
        .is_some_and(|arguments| !arguments.is_empty())
    {
        return Err(ErrorData::invalid_params(
            "choose_quantity accepts empty arguments",
            None,
        ));
    }
    Ok(())
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

// The consumer owns this pipeline. A production server authorizes the selected
// caller/tool and applies quotas here before either form flow or settings effects.
// This demo deliberately serves public data and process-local preferences.
struct ControlledServer(Server);

impl Service<RoleServer> for ControlledServer {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, ErrorData> {
        let id = context.id.clone();
        let result = async {
            check_cancelled(&context)?;
            if let ClientRequest::CallToolRequest(call) = &request
                && call.params.name == FORM_TOOL
                && mrtr::is_mrtr_request(&call.params, &context)?
            {
                return mrtr::handle_tool_call(
                    call.params.clone(),
                    context,
                    &self.0.info.server_info,
                    &ServerHandler::supported_protocol_versions(&self.0),
                    |params, context| self.0.choose_quantity(params, context),
                )
                .await;
            }
            self.0.handle_request(request, context).await
        }
        .await;
        // Stderr leaves STDIO JSON-RPC intact. Do not log arguments/client metadata.
        eprintln!(
            "request {id}: {}",
            if result.is_ok() { "success" } else { "error" }
        );
        result
    }
    async fn handle_notification(
        &self,
        notification: ClientNotification,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.0.handle_notification(notification, context).await
    }
    fn get_info(&self) -> ServerConfig {
        ServerHandler::get_info(&self.0)
    }
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        ServerHandler::supported_protocol_versions(&self.0)
    }
}

pub(super) async fn run(use_router: bool) -> Result<(), Box<dyn std::error::Error>> {
    let service = ControlledServer(Server::new(use_router)?)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

// Keep this path outside the inline test module: there is no support/tests folder.
#[cfg(test)]
#[path = "../../tests/support/wire.rs"]
mod wire;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    async fn request(
        rpc: &mut wire::Rpc,
        modern: bool,
        id: u64,
        method: &str,
        params: Value,
    ) -> Value {
        if modern {
            rpc.modern(id, method, params).await
        } else {
            rpc.request(id, method, params).await
        }
    }

    async fn exercise(modern: bool) {
        let expected_schema = TypedForm::<Answer>::new().unwrap().as_json().clone();
        let (server_io, client_io) = tokio::io::duplex(65536);
        let server = tokio::spawn(
            ControlledServer(Server::new(super::super::USE_ROUTER).unwrap()).serve(server_io),
        );
        let mut rpc = wire::Rpc::new(client_io);
        let discovery = if modern {
            rpc.modern(1, "server/discover", json!({})).await
        } else {
            let discovery = rpc
                .request(
                    1,
                    "initialize",
                    json!({
                        "protocolVersion":"2025-11-25",
                        "capabilities":{"extensions":{"openai/elicitation":{"form":{}}}},
                        "clientInfo":{"name":"example-host","version":"1"}
                    }),
                )
                .await;
            rpc.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .await;
            discovery
        };
        let server = server.await.unwrap().unwrap();
        assert_eq!(
            discovery["result"]["capabilities"]["extensions"]["openai/settings"],
            json!({"readTool":"settings.read","updateTool":"settings.update"})
        );
        let tools = request(&mut rpc, modern, 2, "tools/list", json!({})).await;
        let tools = tools["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert!(tools.iter().any(|tool| tool["name"] == FORM_TOOL));
        let read = request(
            &mut rpc,
            modern,
            3,
            "tools/call",
            json!({"name":"settings.read","arguments":{}}),
        )
        .await;
        assert_eq!(
            read["result"]["structuredContent"]["values"],
            json!({"showGrid":true,"units":"mm","defaultQuantity":2})
        );
        let update = request(
            &mut rpc,
            modern,
            4,
            "tools/call",
            json!({
                "name":"settings.update","arguments":{"set":{"showGrid":false,"defaultQuantity":0}}
            }),
        )
        .await;
        assert_eq!(
            update["result"]["structuredContent"]["values"],
            json!({"showGrid":false,"units":"mm","defaultQuantity":0})
        );
        let invalid = request(
            &mut rpc,
            modern,
            5,
            "tools/call",
            json!({
                "name":"settings.update","arguments":{"set":{}}
            }),
        )
        .await;
        assert_eq!(invalid["error"]["code"], -32602);
        let read = request(
            &mut rpc,
            modern,
            6,
            "tools/call",
            json!({"name":"settings.read","arguments":{}}),
        )
        .await;
        assert_eq!(
            read["result"]["structuredContent"]["values"],
            update["result"]["structuredContent"]["values"]
        );

        for (index, answer) in [
            json!({"action":"accept","content":{"partCode":"AB-123","quantity":2,"units":"mm"}}),
            json!({"action":"accept","content":{"partCode":"AB-123","quantity":4,"units":"in","note":"Preview only"}}),
            json!({"action":"decline"}),
            json!({"action":"cancel"}),
        ].into_iter().enumerate() {
            let id = 7 + 2 * index as u64;
            let result = if modern {
                let initial = rpc.modern(id, "tools/call", json!({"name":FORM_TOOL,"arguments":{}})).await;
                assert_eq!(initial["result"]["resultType"], "input_required");
                assert_eq!(initial["result"]["inputRequests"]["answer"]["params"]["requestedSchema"],
                    expected_schema);
                rpc.modern(id + 1, "tools/call", json!({
                    "name":FORM_TOOL,"arguments":{},"requestState":initial["result"]["requestState"],
                    "inputResponses":{"answer":answer}
                })).await
            } else {
                rpc.legacy_answer = Some(answer.clone());
                rpc.request(id, "tools/call", json!({"name":FORM_TOOL,"arguments":{}})).await
            };
            assert_eq!(result["result"]["structuredContent"], answer);
            if modern { assert_eq!(result["result"]["resultType"], "complete"); }
        }
        if modern {
            assert!(rpc.legacy_requests.is_empty());
        } else {
            assert_eq!(rpc.legacy_requests.len(), 4);
            for request in rpc.legacy_requests {
                assert_eq!(request["params"]["requestedSchema"], expected_schema);
            }
        }
        server.cancel().await.unwrap();
    }

    #[tokio::test]
    async fn legacy_host_can_use_typed_settings_and_forms() {
        tokio::time::timeout(std::time::Duration::from_secs(10), exercise(false))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn modern_host_can_use_typed_settings_and_forms() {
        tokio::time::timeout(std::time::Duration::from_secs(10), exercise(true))
            .await
            .unwrap();
    }
}
