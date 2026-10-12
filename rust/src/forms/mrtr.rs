//! Lossless OpenAI form requests within modern MCP tool round trips.
//!
//! [`FormService`] adapts explicitly registered modern form calls and delegates
//! all other requests to the application's existing rmcp service. The application
//! owns tool declarations, authorization and continuation state; this module
//! neither persists state nor treats an echoed handle as proof of authorization.

use std::{borrow::Cow, collections::BTreeMap, future::Future, pin::Pin};

use rmcp::{
    ErrorData, RoleServer, Service,
    model::{
        CallToolRequestParams, CallToolResult, ClientNotification, ClientRequest, CustomResult,
        Implementation, MetaObject, ProtocolVersion, ResultType, ServerConfig, ServerResult,
    },
    service::{NotificationContext, RequestContext},
};
use serde_json::{Value, json};

use super::{FormResult, OpenAIForm, supports_forms, wire};
use crate::{
    Error,
    error::{invalid, nonblank},
};

/// An initial call or a validated answer from a later tool round trip.
#[derive(Debug, Clone, PartialEq)]
pub enum FormInput {
    /// Neither `inputResponses` nor `requestState` was supplied.
    Initial,
    /// The requested response was decoded and validated, without changing it.
    Response {
        /// Untrusted opaque state. Verify caller/argument binding, integrity,
        /// expiry and single consumption before performing an application effect.
        request_state: String,
        /// Accept, decline or cancel; only accept carries validated content.
        result: FormResult,
    },
}

/// Read one keyed form response from the original tool parameters.
///
/// Missing state/response, an absent key, malformed actions or invalid accepted
/// content are errors. No coercion/default insertion or state lookup occurs.
/// Other response keys remain available for tools requesting multiple inputs:
/// validate the complete expected set before applying any effect. Use the form
/// definition bound to the saved continuation, rather than trusting client data.
pub fn read_input(
    params: &CallToolRequestParams,
    key: &str,
    form: &OpenAIForm,
) -> Result<FormInput, Error> {
    let input = read_response(params, key)?;
    if let FormInput::Response { result, .. } = &input {
        form.validate_result(result)?;
    }
    Ok(input)
}

pub(super) fn read_response(params: &CallToolRequestParams, key: &str) -> Result<FormInput, Error> {
    nonblank(key, "form.input_key")?;
    match (&params.input_responses, &params.request_state) {
        (None, None) => Ok(FormInput::Initial),
        (Some(responses), Some(state)) => {
            let value = responses
                .get(key)
                .ok_or_else(|| invalid(format!("inputResponses is missing form response {key}")))?;
            let result: FormResult = serde_json::from_value(value.clone())?;
            Ok(FormInput::Response {
                request_state: state.clone(),
                result,
            })
        }
        _ => Err(invalid(
            "form continuation requires both inputResponses and requestState",
        )),
    }
}

/// Validated rich-form requests and an application-owned opaque state handle.
///
/// The schema is copied without conversion through rmcp's standard elicitation
/// types. The application must bind the handle to its caller, arguments and form
/// definitions, and arrange expiry/consumption; this builder provides no storage.
#[derive(Debug, Clone)]
pub struct FormInputRequired {
    request_state: String,
    input_requests: BTreeMap<String, wire::InputRequest>,
    meta: Option<MetaObject>,
}

impl FormInputRequired {
    /// Start with one form. Blank keys/messages are errors; state is opaque and
    /// is retained exactly, including an empty string if the application uses it.
    pub fn new(
        request_state: impl Into<String>,
        key: impl Into<String>,
        message: impl Into<String>,
        form: &OpenAIForm,
    ) -> Result<Self, Error> {
        let mut result = Self {
            request_state: request_state.into(),
            input_requests: BTreeMap::new(),
            meta: None,
        };
        result.add_form(key, message, form)?;
        Ok(result)
    }

    /// Add another independently keyed form. Duplicate keys fail without
    /// replacing a previous request. Validate every answer before any effect.
    pub fn add_form(
        &mut self,
        key: impl Into<String>,
        message: impl Into<String>,
        form: &OpenAIForm,
    ) -> Result<(), Error> {
        let key = key.into();
        let message = message.into();
        nonblank(&key, "form.input_key").map_err(Error::configuration)?;
        nonblank(&message, "form.message").map_err(Error::configuration)?;
        if self.input_requests.contains_key(&key) {
            return Err(invalid(format!("duplicate form input key: {key}")).configuration());
        }
        self.input_requests
            .insert(key, wire::InputRequest::new(message, form));
        Ok(())
    }

    /// Attach optional protocol metadata, preserving its namespaces.
    pub fn with_meta(mut self, meta: MetaObject) -> Self {
        self.meta = Some(meta);
        self
    }

    /// Inspect the exact outgoing result without submitting it to a peer.
    pub fn as_json(&self) -> Value {
        serde_json::to_value(wire::InputRequired {
            result_type: ResultType::INPUT_REQUIRED,
            input_requests: &self.input_requests,
            request_state: &self.request_state,
            meta: self.meta.as_ref(),
        })
        .expect("validated form envelope contains only JSON values")
    }
}

/// A modern form handler's completion or its request for further user input.
#[derive(Debug, Clone)]
pub enum FormToolResponse {
    /// A terminal ordinary tool result.
    Complete(CallToolResult),
    /// One or more rich forms; emitted losslessly after the service's gates.
    InputRequired(FormInputRequired),
}

impl From<CallToolResult> for FormToolResponse {
    fn from(value: CallToolResult) -> Self {
        Self::Complete(value)
    }
}

impl From<FormInputRequired> for FormToolResponse {
    fn from(value: FormInputRequired) -> Self {
        Self::InputRequired(value)
    }
}

/// Select the flow for a declared form tool without consuming its request context.
///
/// Modern requests return `true`; ordinary legacy calls return `false`. Legacy
/// calls carrying continuation fields are rejected before application effects.
/// This does not validate modern capabilities: [`handle_tool_call`] performs
/// those gates before its callback. The consumer still chooses and authorizes
/// the tool. Do not use this as a selector for unrelated tools.
pub fn is_mrtr_request(
    params: &CallToolRequestParams,
    context: &RequestContext<RoleServer>,
) -> Result<bool, ErrorData> {
    if context
        .protocol_version()
        .is_some_and(|version| version.as_str() >= "2026-07-28")
    {
        return Ok(true);
    }
    if params.input_responses.is_some() || params.request_state.is_some() {
        return Err(ErrorData::invalid_request(
            "MRTR form continuation requires MCP 2026-07-28 or later",
            None,
        ));
    }
    Ok(false)
}

/// Execute a modern form callback inside the consumer's own request pipeline.
///
/// Call after selecting and authorizing the tool. Protocol version, required
/// request metadata and both core and OpenAI form capabilities are validated
/// before the callback.
/// Parameters and context are passed unchanged. The callback can borrow server
/// state and owns argument validation, authorization and secure continuations.
/// The returned lossless result includes the supplied server implementation
/// information, preserving other metadata. Errors are returned to the consumer
/// so it can record diagnostics together with success and input-required replies.
/// Legacy calls are rejected; select their normal handler with [`is_mrtr_request`].
pub async fn handle_tool_call<F, Fut>(
    params: CallToolRequestParams,
    context: RequestContext<RoleServer>,
    server_info: &Implementation,
    supported_versions: &[ProtocolVersion],
    handler: F,
) -> Result<ServerResult, ErrorData>
where
    F: FnOnce(CallToolRequestParams, RequestContext<RoleServer>) -> Fut,
    Fut: Future<Output = Result<FormToolResponse, ErrorData>>,
{
    if !is_mrtr_request(&params, &context)? {
        return Err(ErrorData::invalid_request(
            "MRTR form handling requires MCP 2026-07-28 or later",
            None,
        ));
    }
    validate_context(&context, supported_versions)?;
    let mut response = handler(params, context).await?;
    let meta = match &mut response {
        FormToolResponse::Complete(result) => {
            result.result_type = Some(ResultType::COMPLETE);
            &mut result.meta
        }
        FormToolResponse::InputRequired(result) => &mut result.meta,
    };
    // The consumer's implementation identity is authoritative for this key.
    meta.get_or_insert_default().0.insert(
        "io.modelcontextprotocol/serverInfo".into(),
        json!(server_info),
    );
    match response {
        FormToolResponse::Complete(result) => Ok(ServerResult::CallToolResult(result)),
        FormToolResponse::InputRequired(result) => Ok(ServerResult::CustomResult(
            CustomResult::new(result.as_json()),
        )),
    }
}

type HandlerFuture = Pin<Box<dyn Future<Output = Result<FormToolResponse, ErrorData>> + Send>>;
type Handler =
    Box<dyn Fn(CallToolRequestParams, RequestContext<RoleServer>) -> HandlerFuture + Send + Sync>;

/// Modern form callbacks around an existing rmcp service.
///
/// Register the same tool names in the underlying server's `tools/list` and
/// legacy router. Only modern calls to these names are intercepted. Callbacks
/// receive the original parameters/context, including arguments, state and the
/// cancellation token. They must authorize the caller and validate arguments.
/// The adapter requires complete modern per-request metadata and validates the
/// protocol and both core/OpenAI form capabilities before invoking them.
/// It provides no task-result variant.
/// Modern results include the underlying server's implementation information,
/// preserving other result metadata.
///
/// This adapter intercepts registered modern calls before the inner request
/// handler. If common controls or diagnostics live there, use [`handle_tool_call`]
/// inside that pipeline instead, or wrap this entire adapter with your controls.
pub struct FormService<S> {
    inner: S,
    handlers: BTreeMap<String, Handler>,
}

impl<S> FormService<S> {
    /// Wrap an existing service without changing its configuration or routes.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            handlers: BTreeMap::new(),
        }
    }

    /// Register a modern callback for an already declared tool.
    /// Blank/duplicate names fail before changing this adapter. This does not
    /// add a declaration or legacy route to the underlying server.
    pub fn register<F, Fut>(&mut self, name: impl Into<String>, handler: F) -> Result<(), Error>
    where
        F: Fn(CallToolRequestParams, RequestContext<RoleServer>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<FormToolResponse, ErrorData>> + Send + 'static,
    {
        let name = name.into();
        nonblank(&name, "form.tool_name").map_err(Error::configuration)?;
        if self.handlers.contains_key(&name) {
            return Err(Error::ToolCollision(name));
        }
        self.handlers.insert(
            name,
            Box::new(move |params, context| Box::pin(handler(params, context))),
        );
        Ok(())
    }
}

impl<S: Service<RoleServer>> Service<RoleServer> for FormService<S> {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, ErrorData> {
        if let ClientRequest::CallToolRequest(call) = &request
            && let Some(handler) = self.handlers.get(call.params.name.as_ref())
            && is_mrtr_request(&call.params, &context)?
        {
            return handle_tool_call(
                call.params.clone(),
                context,
                &self.inner.get_info().server_info,
                &self.inner.supported_protocol_versions(),
                handler,
            )
            .await;
        }
        self.inner.handle_request(request, context).await
    }

    async fn handle_notification(
        &self,
        notification: ClientNotification,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.inner.handle_notification(notification, context).await
    }

    fn get_info(&self) -> ServerConfig {
        self.inner.get_info()
    }
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        self.inner.supported_protocol_versions()
    }
}

fn validate_context(
    context: &RequestContext<RoleServer>,
    supported: &[ProtocolVersion],
) -> Result<(), ErrorData> {
    if let Some(requested) = context.meta.protocol_version()
        && !supported.contains(&requested)
    {
        return Err(ErrorData::unsupported_protocol_version(
            requested, supported,
        ));
    }
    // rmcp's inline-lifecycle marker is private. Require the self-contained
    // 2026-07-28 request contract for every modern callback, without relying on
    // session capability fallback. Legacy calls still use the inner service.
    let missing = context
        .meta
        .missing_required_keys(&ProtocolVersion::V_2026_07_28);
    if !missing.is_empty() {
        return Err(ErrorData::invalid_params(
            format!(
                "request _meta is missing or has malformed required fields: {}",
                missing.join(", ")
            ),
            None,
        ));
    }
    if !context
        .client_capabilities()
        .as_ref()
        .is_some_and(|capabilities| {
            supports_forms(capabilities)
                && capabilities
                    .elicitation
                    .as_ref()
                    .is_some_and(|elicitation| elicitation.form.is_some())
        })
    {
        return Err(ErrorData::missing_required_client_capability(
            serde_json::from_value(json!({
                "elicitation":{"form":{}},
                "extensions":{"openai/elicitation":{"form":{}}}
            }))
            .expect("valid form capabilities"),
        ));
    }
    Ok(())
}
