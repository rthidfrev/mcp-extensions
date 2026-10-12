//! A macro-free custom tool registry and consumer-controlled form request pipeline.
use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

use openai_mcp_extensions::{
    Error,
    forms::{
        self, OpenAIForm,
        mrtr::{self, FormInput, FormInputRequired, FormToolResponse},
    },
    mentions::{self, MentionSearchResult},
    settings::{SettingsCapability, SettingsRegistration},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, Service, ServiceExt,
    handler::server::{router::tool::ToolRoute, tool::ToolCallContext},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ClientNotification, ClientRequest,
        ListToolsResult, ProtocolVersion, ServerConfig, ServerResult, Tool,
    },
    service::{NotificationContext, RequestContext},
};
use serde_json::json;
use tokio::sync::Mutex;

struct Server {
    tools: BTreeMap<String, ToolRoute<Self>>,
    settings: SettingsCapability,
    form: Arc<OpenAIForm>,
}

impl Server {
    fn new() -> Result<Self, Error> {
        let mut tools: BTreeMap<String, ToolRoute<Self>> = BTreeMap::new();
        let ordinary = ToolRoute::new_dyn(
            Tool::new(
                "status",
                "Read server status",
                json!({"type":"object"}).as_object().unwrap().clone(),
            ),
            |_| Box::pin(async { Ok(CallToolResult::structured(json!({"ready":true})).into()) }),
        );
        tools.insert(ordinary.name().into(), ordinary);

        let mentions = mentions::search_tool(|_, _| async { Ok(MentionSearchResult::default()) });
        if tools.contains_key(mentions.name()) {
            return Err(Error::ToolCollision(mentions.name().into()));
        }
        tools.insert(mentions.name().into(), mentions);

        let values = Arc::new(Mutex::new(
            json!({"enabled":true}).as_object().unwrap().clone(),
        ));
        let read_values = values.clone();
        let pair = SettingsRegistration::new(json!({"type":"object","properties":{
            "enabled":{"type":"boolean","title":"Enabled"}
        }}))?
        .into_tools(
            move |_| {
                let values = read_values.clone();
                async move { Ok(values.lock().await.clone()) }
            },
            move |set, _| {
                let values = values.clone();
                async move {
                    let mut values = values.lock().await;
                    values.extend(set);
                    Ok(values.clone())
                }
            },
        );
        let (routes, settings) = pair.into_parts();
        // This registry has no disabled entries. Check the complete pair before
        // mutation; there are no fallible operations between these two inserts.
        for route in &routes {
            if tools.contains_key(route.name()) {
                return Err(Error::ToolCollision(route.name().into()));
            }
        }
        for route in routes {
            tools.insert(route.name().into(), route);
        }

        let form = Arc::new(OpenAIForm::from_json(
            json!({"type":"object","properties":{
            "confirmed":{"type":"boolean","title":"Confirm"}
        },"required":["confirmed"]}),
        )?);
        let legacy_form = form.clone();
        let route = ToolRoute::new_dyn(
            Tool::new(
                "confirm",
                "Read a confirmation",
                json!({"type":"object","additionalProperties":false})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            move |context| {
                let form = legacy_form.clone();
                Box::pin(async move {
                    validate_form_arguments(&context.arguments)?;
                    let response = forms::elicit_input(
                        &context.request_context,
                        "Confirm this read-only example",
                        &form,
                    )
                    .await
                    .map_err(ErrorData::from)?;
                    Ok(CallToolResult::structured(json!(response)).into())
                })
            },
        );
        if tools.contains_key(route.name()) {
            return Err(Error::ToolCollision(route.name().into()));
        }
        tools.insert(route.name().into(), route);
        Ok(Self {
            tools,
            settings,
            form,
        })
    }

    async fn confirm(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<FormToolResponse, ErrorData> {
        validate_form_arguments(&params.arguments)?;
        if context.ct.is_cancelled() {
            return Err(ErrorData::invalid_request("request cancelled", None));
        }
        match mrtr::read_input(&params, "answer", &self.form).map_err(ErrorData::from)? {
            FormInput::Initial => Ok(FormInputRequired::new(
                "confirmation-v1",
                "answer",
                "Confirm this read-only example",
                &self.form,
            )
            .map_err(ErrorData::from)?
            .into()),
            FormInput::Response {
                request_state,
                result,
            } => {
                // Stateless, public, read-only demonstration: no write or private
                // data depends on this version marker. Mutating tools need secure,
                // caller-bound, expiring and atomically consumed application state.
                if request_state != "confirmation-v1" {
                    return Err(ErrorData::invalid_params(
                        "unknown confirmation state",
                        None,
                    ));
                }
                Ok(CallToolResult::structured(json!(result)).into())
            }
        }
    }
}

fn validate_form_arguments(arguments: &Option<rmcp::model::JsonObject>) -> Result<(), ErrorData> {
    if arguments
        .as_ref()
        .is_some_and(|arguments| !arguments.is_empty())
    {
        return Err(ErrorData::invalid_params(
            "confirm accepts empty arguments",
            None,
        ));
    }
    Ok(())
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.server_info.name = "custom-registry-example".into();
        // Both settings routes are already installed. Construction alone would
        // not justify advertising their capability.
        self.settings.apply_capabilities(&mut info.capabilities);
        mentions::apply_capabilities(&mut info.capabilities);
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
            self.tools.values().map(|tool| tool.attr.clone()).collect(),
        ))
    }
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let route = self
            .tools
            .get(params.name.as_ref())
            .ok_or_else(|| ErrorData::invalid_params("tool not found", None))?;
        (route.call)(ToolCallContext::new(self, params, context)).await
    }
}

// This is the consumer's request pipeline, not an SDK requirement. Put real
// authorization/quotas here before either ordinary or modern form execution.
struct ControlledServer(Server);

impl Service<RoleServer> for ControlledServer {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, ErrorData> {
        let id = context.id.clone();
        let result = async {
            if context.ct.is_cancelled() {
                return Err(ErrorData::invalid_request("request cancelled", None));
            }
            if let ClientRequest::CallToolRequest(call) = &request
                && call.params.name == "confirm"
                && mrtr::is_mrtr_request(&call.params, &context)?
            {
                return mrtr::handle_tool_call(
                    call.params.clone(),
                    context,
                    &ServerHandler::get_info(&self.0).server_info,
                    &ServerHandler::supported_protocol_versions(&self.0),
                    |params, context| self.0.confirm(params, context),
                )
                .await;
            }
            self.0.handle_request(request, context).await
        }
        .await;
        // Stderr leaves stdio JSON-RPC intact; omit arguments and metadata.
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = ControlledServer(Server::new()?)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/support/wire.rs"]
mod wire;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn custom_example_serves_tools_and_forms_on_both_protocols() {
        for modern in [false, true] {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let (server_io, client_io) = tokio::io::duplex(65536);
                let server = tokio::spawn(ControlledServer(Server::new().unwrap()).serve(server_io));
                let mut rpc = wire::Rpc::new(client_io);
                if modern { rpc.modern(1, "server/discover", json!({})).await; }
                else {
                    rpc.request(1, "initialize", json!({"protocolVersion":"2025-11-25","capabilities":{"extensions":{"openai/elicitation":{"form":{}}}},"clientInfo":{"name":"example-host","version":"1"}})).await;
                    rpc.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
                }
                let server = server.await.unwrap().unwrap();
                let tools = if modern { rpc.modern(2, "tools/list", json!({})).await } else { rpc.request(2, "tools/list", json!({})).await };
                assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 5);
                let params = json!({"name":"confirm","arguments":{}});
                if modern {
                    let initial = rpc.modern(3, "tools/call", params).await;
                    assert_eq!(initial["result"]["resultType"], "input_required");
                    let result = rpc.modern(4, "tools/call", json!({"name":"confirm","arguments":{},"requestState":initial["result"]["requestState"],"inputResponses":{"answer":{"action":"accept","content":{"confirmed":true}}}})).await;
                    assert_eq!(result["result"]["structuredContent"]["content"]["confirmed"], true);
                } else {
                    rpc.legacy_answer = Some(json!({"action":"decline"}));
                    let result = rpc.request(3, "tools/call", params).await;
                    assert_eq!(result["result"]["structuredContent"]["action"], "decline");
                }
                server.cancel().await.unwrap();
            }).await.unwrap();
        }
    }
}
