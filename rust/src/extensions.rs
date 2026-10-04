use rmcp::{
    ErrorData, RoleServer,
    handler::server::router::tool::{ToolRoute, ToolRouter},
    model::{
        CallToolResult, JsonObject, MetaObject, ServerCapabilities, Tool, ToolAnnotations,
        ToolsCapability,
    },
    service::RequestContext,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, sync::Arc};

use crate::{
    Error,
    mentions::{MentionSearchParams, MentionSearchResult, SEARCH_MENTIONS_TOOL},
    settings::{SETTINGS_CAPABILITY_KEY, SettingsCapability, SettingsRegistration},
};

/// Registers OpenAI extension routes on an existing rmcp server.
/// Configure routes before serving; advertise the resulting capabilities in `get_info`.
#[derive(Debug, Default)]
pub struct OpenAIExtensions {
    settings: Option<SettingsCapability>,
    has_tools: bool,
}

impl OpenAIExtensions {
    /// An empty extension registration; no tools are installed automatically.
    pub fn new() -> Self {
        Self::default()
    }

    /// Install `search_mentions`. Empty queries are passed through unchanged.
    /// Rejects name collisions before mutating the router; handler results are validated.
    /// The context lets the application authorize the request for its current user.
    pub fn register_mentions<S, F, Fut>(
        &mut self,
        router: &mut ToolRouter<S>,
        handler: F,
    ) -> Result<(), Error>
    where
        S: Send + Sync + 'static,
        F: Fn(MentionSearchParams, RequestContext<RoleServer>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<MentionSearchResult, ErrorData>> + Send + 'static,
    {
        if router.has_route(SEARCH_MENTIONS_TOOL) {
            return Err(Error::ToolCollision(SEARCH_MENTIONS_TOOL.into()));
        }
        let mut tool = make_tool(
            SEARCH_MENTIONS_TOOL,
            json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
            Some(json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": {"oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "type": {"const": "resource_link"},
                                    "uri": {"type": "string"},
                                    "name": {"type": "string"}
                                },
                                "required": ["type", "uri", "name"]
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "type": {"const": "resource"},
                                    "resourceUri": {"type": "string", "minLength": 1},
                                    "title": {"type": "string", "minLength": 1},
                                    "subtitle": {"type": "string"},
                                    "icons": {"type": "array"}
                                },
                                "required": ["type", "resourceUri", "title"],
                                "additionalProperties": false
                            }
                        ]}
                    }
                },
                "required": ["items"],
                "additionalProperties": false
            })),
            true,
        );
        tool.meta = Some(MetaObject(
            json!({"openai/extensions":{"mentions/search":{}},"ui":{"visibility":["app"]}})
                .as_object()
                .expect("object")
                .clone(),
        ));
        let handler = Arc::new(handler);
        router.add_route(ToolRoute::new_dyn(tool, move |context| {
            let handler = handler.clone();
            Box::pin(async move {
                let params =
                    serde_json::from_value(Value::Object(context.arguments.unwrap_or_default()))
                        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
                let result = handler(params, context.request_context).await?;
                result.validate().map_err(internal)?;
                Ok(structured(
                    serde_json::to_value(result)
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?,
                )
                .into())
            })
        }));
        self.has_tools = true;
        Ok(())
    }

    /// Install a validated settings read/update pair atomically.
    /// `read` must authorize and return all effective values. `update` receives
    /// validated replacements and must authorize, persist atomically, and only
    /// then return all effective values. The SDK does not provide persistence.
    /// Registration failures leave both the router and capabilities unchanged.
    pub fn register_settings<S, R, U, RFut, UFut>(
        &mut self,
        router: &mut ToolRouter<S>,
        registration: SettingsRegistration,
        read: R,
        update: U,
    ) -> Result<(), Error>
    where
        S: Send + Sync + 'static,
        R: Fn(RequestContext<RoleServer>) -> RFut + Send + Sync + 'static,
        U: Fn(JsonObject, RequestContext<RoleServer>) -> UFut + Send + Sync + 'static,
        RFut: Future<Output = Result<JsonObject, ErrorData>> + Send + 'static,
        UFut: Future<Output = Result<JsonObject, ErrorData>> + Send + 'static,
    {
        if self.settings.is_some() {
            return Err(crate::error::invalid("settings already registered"));
        }
        for name in [
            &registration.capability.read_tool,
            &registration.capability.update_tool,
        ] {
            if router.has_route(name) {
                return Err(Error::ToolCollision(name.clone()));
            }
        }
        let capability = registration.capability.clone();
        let values_schema = json!({
            "type": "object",
            "properties": registration.schema["properties"],
            "required": registration.schema["properties"].as_object().expect("validated").keys().collect::<Vec<_>>(),
            "additionalProperties": false
        });
        let read_output = json!({
            "type": "object",
            "properties": {
                "schema": {"type": "object", "const": registration.schema},
                "layout": {"type": "array", "items": {"type":"object"}},
                "values": values_schema
            },
            "required": ["schema", "values"]
        });
        let mut read_output = read_output;
        if let Some(layout) = &registration.layout {
            read_output["properties"]["layout"]["const"] = json!(layout);
        }
        let read_tool = make_tool(
            &capability.read_tool,
            json!({"type":"object","properties":{},"additionalProperties":false}),
            Some(read_output),
            true,
        );
        let update_tool = make_tool(
            &capability.update_tool,
            json!({
                "type": "object",
                "properties": {
                    "set": {
                        "type": "object",
                        "properties": registration.schema["properties"],
                        "additionalProperties": false,
                        "minProperties": 1
                    }
                },
                "required": ["set"],
                "additionalProperties": false
            }),
            Some(
                json!({"type":"object","properties":{"values":values_schema},"required":["values"]}),
            ),
            false,
        );
        let registration = Arc::new(registration);
        let read_registration = registration.clone();
        let read = Arc::new(read);
        let read_route = ToolRoute::new_dyn(read_tool, move |context| {
            let registration = read_registration.clone();
            let read = read.clone();
            Box::pin(async move {
                if context.arguments.as_ref().is_some_and(|a| !a.is_empty()) {
                    return Err(ErrorData::invalid_params(
                        "settings read arguments must be empty",
                        None,
                    ));
                }
                let values = read(context.request_context).await?;
                registration.validate_values(&values).map_err(internal)?;
                let mut content = json!({"schema":registration.schema,"values":values});
                if let Some(layout) = &registration.layout {
                    content["layout"] = serde_json::to_value(layout)
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                }
                Ok(structured(content).into())
            })
        });
        let update = Arc::new(update);
        let update_route = ToolRoute::new_dyn(update_tool, move |context| {
            let registration = registration.clone();
            let update = update.clone();
            Box::pin(async move {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Args {
                    set: JsonObject,
                }
                let args: Args =
                    serde_json::from_value(Value::Object(context.arguments.unwrap_or_default()))
                        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
                registration
                    .validate_patch(&args.set)
                    .map_err(ErrorData::from)?;
                let values = update(args.set, context.request_context).await?;
                registration.validate_values(&values).map_err(internal)?;
                Ok(structured(json!({"values":values})).into())
            })
        });
        router.add_route(read_route);
        router.add_route(update_route);
        self.settings = Some(capability);
        self.has_tools = true;
        Ok(())
    }

    /// Add tool discovery and settings capability advertisements, preserving
    /// unrelated capabilities. Call this in the application's `get_info`.
    /// Settings are advertised in both `extensions` and legacy `experimental`.
    pub fn apply_capabilities(&self, capabilities: &mut ServerCapabilities) {
        if self.has_tools && capabilities.tools.is_none() {
            capabilities.tools = Some(ToolsCapability::default());
        }
        if let Some(settings) = &self.settings {
            let value = json!({"readTool":settings.read_tool,"updateTool":settings.update_tool})
                .as_object()
                .expect("object")
                .clone();
            capabilities
                .extensions
                .get_or_insert_with(Default::default)
                .insert(SETTINGS_CAPABILITY_KEY.into(), value.clone());
            capabilities
                .experimental
                .get_or_insert_with(Default::default)
                .insert(SETTINGS_CAPABILITY_KEY.into(), value);
        }
    }
}

fn make_tool(name: &str, input: Value, output: Option<Value>, readonly: bool) -> Tool {
    let mut tool = Tool::new(
        name.to_owned(),
        name.to_owned(),
        Arc::new(input.as_object().expect("object schema").clone()),
    );
    tool.output_schema = output.map(|v| Arc::new(v.as_object().expect("object schema").clone()));
    let mut annotations = ToolAnnotations::default();
    annotations.read_only_hint = Some(readonly);
    tool.annotations = Some(annotations);
    tool
}
fn structured(value: Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![]);
    result.structured_content = Some(value);
    result
}
fn internal(error: Error) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}
