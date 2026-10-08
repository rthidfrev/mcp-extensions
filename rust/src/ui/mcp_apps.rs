//! Optional generic MCP Apps conveniences; OpenAI annotations live in the parent module.
use mcp_apps_server::{ToolUi, UiResource, UiResourceUri};
use rmcp::model::{ClientCapabilities, MetaObject, ResourceContents};

use crate::Error;

pub use mcp_apps_server::{UI_EXTENSION_ID as APP_CAPABILITY_KEY, UI_MIME_TYPE as APP_MIME_TYPE};

/// Whether a client advertises support for HTML MCP Apps (`mcp-apps` feature).
/// This does not establish which OpenAI entrypoints that host implements.
/// Absent or malformed declarations return false. Use
/// [`mcp_apps_server::ui_capability`] to distinguish these cases and retain errors.
pub fn supports_mcp_apps(capabilities: &ClientCapabilities) -> bool {
    mcp_apps_server::ui_capability(capabilities)
        .ok()
        .flatten()
        .is_some_and(|ui| ui.supports_html())
}

/// Associate a tool's metadata with an existing `ui://` resource (`mcp-apps` feature).
/// Delegates to [`ToolUi::merge_into`], preserving UI visibility and unrelated
/// metadata. Conflicting associations, including deprecated `ui/resourceUri`,
/// return an error without mutation. Equal associations are accepted.
/// The application must serve the referenced resource.
/// This compatibility helper retains its rejection of the empty `ui://` identifier;
/// use [`UiResourceUri`] directly for the generic MCP Apps URI contract.
///
/// # Errors
///
/// Returns [`Error::Configuration`] for an invalid URI, non-object `ui` metadata
/// or a conflicting association. The original MCP Apps error remains its source.
pub fn associate_app_resource(meta: &mut MetaObject, uri: &str) -> Result<(), Error> {
    ToolUi::new(app_uri(uri)?)
        .merge_into(meta)
        .map_err(|error| Error::from(error).configuration())
}

/// Build ordinary rmcp HTML resource contents (`mcp-apps` feature).
/// Metadata, including `ui.csp` and `openai/ui`, is preserved on these contents.
/// This does not register a resource handler or implement a browser runtime.
/// For a descriptor and matching contents, use [`UiResource`] directly; for
/// registration and reads, use [`mcp_apps_server::AppRouter`]. This compatibility
/// helper retains its rejection of the empty `ui://` identifier.
///
/// # Errors
///
/// Returns [`Error::Configuration`] if the URI is not a valid `ui://` resource URI.
pub fn app_html_resource(
    uri: &str,
    html: impl Into<String>,
    meta: MetaObject,
) -> Result<ResourceContents, Error> {
    Ok(UiResource::new(app_uri(uri)?, uri)
        .html_contents(html)
        .with_meta(meta))
}

fn app_uri(uri: &str) -> Result<UiResourceUri, Error> {
    // Preserve the earlier helper's stricter identifier rule without duplicating
    // the generic URI parser. Typed consumers use the dependency's contract.
    if uri == "ui://" {
        return Err(
            crate::error::invalid("MCP App resource URI must not be empty").configuration(),
        );
    }
    UiResourceUri::new(uri).map_err(|error| Error::from(error).configuration())
}
