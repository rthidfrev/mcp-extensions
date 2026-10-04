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
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientNotification,
        ClientRequest, CustomResult, MetaObject, ProtocolVersion, ResultType, ServerConfig,
        ServerResult,
    },
    service::{NotificationContext, RequestContext},
};
use serde_json::{Value, json};

use super::{
    ELICITATION_CAPABILITY_KEY, ELICITATION_METHOD, FormResult, OpenAIForm, supports_forms,
};
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
    nonblank(key, "form.input_key")?;
    match (&params.input_responses, &params.request_state) {
        (None, None) => Ok(FormInput::Initial),
        (Some(responses), Some(state)) => {
            let value = responses
                .get(key)
                .ok_or_else(|| invalid(format!("inputResponses is missing form response {key}")))?;
            let result: FormResult = serde_json::from_value(value.clone())?;
            form.validate_result(&result)?;
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
    input_requests: BTreeMap<String, Value>,
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
        nonblank(&key, "form.input_key")?;
        nonblank(&message, "form.message")?;
        if self.input_requests.contains_key(&key) {
            return Err(invalid(format!("duplicate form input key: {key}")));
        }
        self.input_requests.insert(
            key,
            json!({
                "method": ELICITATION_METHOD,
                "params": {"mode": "form", "message": message, "requestedSchema": form.as_json()}
            }),
        );
        Ok(())
    }

    /// Attach optional protocol metadata, preserving its namespaces.
    pub fn with_meta(mut self, meta: MetaObject) -> Self {
        self.meta = Some(meta);
        self
    }

    /// Inspect the exact outgoing result without submitting it to a peer.
    pub fn as_json(&self) -> Value {
        let mut value = json!({
            "resultType": "input_required", "inputRequests": self.input_requests,
            "requestState": self.request_state
        });
        if let Some(meta) = &self.meta {
            value["_meta"] = json!(meta);
        }
        value
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
/// protocol/capability before invoking them. It provides no task-result variant.
/// Modern results include the underlying server's implementation information,
/// preserving other result metadata.
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
        nonblank(&name, "form.tool_name")?;
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
        {
            if context
                .protocol_version()
                .is_some_and(|version| version.as_str() >= "2026-07-28")
            {
                validate_context(&context, &self.inner.supported_protocol_versions())?;
                let mut response = handler(call.params.clone(), context).await?;
                let meta = match &mut response {
                    FormToolResponse::Complete(result) => {
                        result.result_type = Some(ResultType::COMPLETE);
                        &mut result.meta
                    }
                    FormToolResponse::InputRequired(result) => &mut result.meta,
                };
                // Modern MCP recommends this information on every result.
                // The service configuration is authoritative for this name;
                // retain all other application-supplied metadata.
                meta.get_or_insert_default().0.insert(
                    "io.modelcontextprotocol/serverInfo".into(),
                    json!(self.inner.get_info().server_info),
                );
                return match response {
                    FormToolResponse::Complete(result) => Ok(ServerResult::CallToolResult(result)),
                    FormToolResponse::InputRequired(result) => Ok(ServerResult::CustomResult(
                        CustomResult::new(result.as_json()),
                    )),
                };
            }
            if call.params.input_responses.is_some() || call.params.request_state.is_some() {
                return Err(ErrorData::invalid_request(
                    "MRTR form continuation requires MCP 2026-07-28 or later",
                    None,
                ));
            }
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
        .is_some_and(supports_forms)
    {
        let mut required = ClientCapabilities::default();
        required.extensions = Some(
            [(
                ELICITATION_CAPABILITY_KEY.into(),
                json!({"form": {}}).as_object().unwrap().clone(),
            )]
            .into(),
        );
        return Err(ErrorData::missing_required_client_capability(required));
    }
    Ok(())
}
