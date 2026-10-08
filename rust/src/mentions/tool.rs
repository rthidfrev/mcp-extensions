//! Standalone mention-search tool construction.
use super::{MentionSearchParams, MentionSearchResult, SEARCH_MENTIONS_TOOL};
use crate::Error;
use crate::extensions::ExtensionFuture;
use crate::extensions::tool::{internal, make_tool, structured};
use rmcp::{
    ErrorData, RoleServer, handler::server::router::tool::ToolRoute, model::MetaObject,
    service::RequestContext,
};
use serde_json::{Value, json};
use std::{future::Future, sync::Arc};

/// Build the complete mention-search tool without installing it.
///
/// The returned rmcp route contains its declaration, input parsing, callback,
/// output validation and serialization. A custom dispatcher can expose `attr`
/// in `tools/list` and invoke `call` with its original `ToolCallContext` after
/// its own checks. No macros or ToolRouter are required. Empty queries are valid;
/// invalid input produces invalid-params errors, invalid results internal errors.
/// Construction neither changes a registry nor advertises server capabilities.
/// The callback owns authorization and receives the unchanged request context.
pub fn search_tool<S, F, Fut>(handler: F) -> ToolRoute<S>
where
    S: Send + Sync + 'static,
    F: Fn(MentionSearchParams, RequestContext<RoleServer>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<MentionSearchResult, ErrorData>> + Send + 'static,
{
    search_tool_with_state(move |_, params, context| Box::pin(handler(params, context)))
}

/// Build a mention-search route whose callback can borrow the current server.
///
/// The callback receives the `ToolCallContext` server reference and unchanged
/// request context. Its boxed `Send` future may borrow that server until the call
/// completes, including across `.await`; the consumer needs no additional `Arc`.
/// The callback itself must be `Send + Sync + 'static` and the server `Send + Sync`.
/// Input parsing, output validation, metadata and errors match [`search_tool`].
/// Authorization remains the callback's responsibility. Construction installs
/// nothing and does not advertise capabilities.
///
/// ```
/// use openai_mcp_extensions::mentions::{search_tool_with_state, MentionSearchResult};
///
/// struct Server { label: String }
/// let route = search_tool_with_state(|server: &Server, _params, _context| {
///     Box::pin(async move {
///         std::future::ready(()).await;
///         // The server is borrowed after the await, not cloned into the callback.
///         assert!(!server.label.is_empty());
///         Ok(MentionSearchResult::default())
///     })
/// });
/// ```
///
/// A future holding a non-`Send` value across `.await` cannot be registered:
///
/// ```compile_fail
/// use openai_mcp_extensions::mentions::{search_tool_with_state, MentionSearchResult};
/// use std::rc::Rc;
/// let route = search_tool_with_state(|_server: &(), _params, _context| {
///     Box::pin(async move {
///         let value = Rc::new(1);
///         std::future::ready(()).await;
///         assert_eq!(*value, 1);
///         Ok(MentionSearchResult::default())
///     })
/// });
/// ```
///
/// Server state must support shared access from a `Send` future:
///
/// ```compile_fail
/// use openai_mcp_extensions::mentions::{search_tool_with_state, MentionSearchResult};
/// use std::cell::Cell;
/// let route = search_tool_with_state(|server: &Cell<u32>, _params, _context| {
///     Box::pin(async move {
///         std::future::ready(()).await;
///         server.set(1);
///         Ok(MentionSearchResult::default())
///     })
/// });
/// ```
pub fn search_tool_with_state<S, F>(handler: F) -> ToolRoute<S>
where
    S: Send + Sync + 'static,
    F: for<'a> Fn(
            &'a S,
            MentionSearchParams,
            RequestContext<RoleServer>,
        ) -> ExtensionFuture<'a, MentionSearchResult>
        + Send
        + Sync
        + 'static,
{
    let mut tool = make_tool(
        SEARCH_MENTIONS_TOOL,
        json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        }),
        Some(output_schema()),
        true,
    );
    tool.meta = Some(MetaObject(
        json!({"openai/extensions":{"mentions/search":{}},"ui":{"visibility":["app"]}})
            .as_object()
            .expect("object")
            .clone(),
    ));
    let handler = Arc::new(handler);
    ToolRoute::new_dyn(tool, move |context| {
        let handler = handler.clone();
        Box::pin(async move {
            let params =
                serde_json::from_value(Value::Object(context.arguments.unwrap_or_default()))
                    .map_err(|source| ErrorData::from(Error::from(source)))?;
            let result = handler(context.service, params, context.request_context).await?;
            result.validate().map_err(internal)?;
            Ok(structured(
                serde_json::to_value(result)
                    .map_err(|source| Error::from(source).into_internal())?,
            )
            .into())
        })
    })
}

// This feature owns the result schema alongside its presentation validation.
// Native Resource fields and metadata stay extensible; the SDK-owned resource
// presentation is closed and its labels must match the runtime nonblank rule.
fn output_schema() -> Value {
    let nonblank = json!({"type":"string","minLength":1,"pattern":r"\S"});
    let icons = json!({
        "type":"array",
        "items": {
            "type":"object",
            "properties": {
                "src":{"type":"string"},
                "mimeType":{"type":"string"},
                "sizes":{"type":"array","items":{"type":"string"}},
                "theme":{"enum":["light","dark"]}
            },
            "required":["src"]
        }
    });
    json!({
        "type":"object",
        "properties": {
            "items": {
                "type":"array",
                "items":{"oneOf":[
                    {
                        "type":"object",
                        "properties": {
                            "type":{"const":"resource_link"},
                            "uri":nonblank,
                            "name":nonblank,
                            "icons":icons
                        },
                        "required":["type","uri","name"]
                    },
                    {
                        "type":"object",
                        "properties": {
                            "type":{"const":"resource"},
                            "resourceUri":nonblank,
                            "title":nonblank,
                            "subtitle":nonblank,
                            "icons":icons
                        },
                        "required":["type","resourceUri","title"],
                        "additionalProperties":false
                    }
                ]}
            }
        },
        "required":["items"],
        "additionalProperties":false
    })
}
