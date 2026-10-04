//! Metadata for launching MCP Apps and choosing their display modes.
use rmcp::model::{ClientCapabilities, Icon, MetaObject, ResourceContents};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Error, error::nonblank};

/// `_meta` key for OpenAI UI annotations.
pub const UI_META_KEY: &str = "openai/ui";

/// MIME type for the HTML resource an MCP App tool references.
pub const APP_MIME_TYPE: &str = "text/html;profile=mcp-app";
/// Client extension capability declaring MCP Apps support.
pub const APP_CAPABILITY_KEY: &str = "io.modelcontextprotocol/ui";

/// Whether a client advertises support for HTML MCP Apps.
/// This does not establish which OpenAI entrypoints that host implements.
pub fn supports_mcp_apps(capabilities: &ClientCapabilities) -> bool {
    capabilities
        .extensions
        .as_ref()
        .and_then(|e| e.get(APP_CAPABILITY_KEY))
        .and_then(|e| e.get("mimeTypes"))
        .and_then(Value::as_array)
        .is_some_and(|types| types.iter().any(|t| t.as_str() == Some(APP_MIME_TYPE)))
}

/// Associate a tool's metadata with an existing `ui://` resource.
/// Preserves existing UI visibility and unrelated metadata, rejecting malformed
/// metadata without mutation. The application must serve the referenced resource.
pub fn associate_app_resource(meta: &mut MetaObject, uri: &str) -> Result<(), Error> {
    validate_app_uri(uri)?;
    let mut ui = match meta.0.get("ui") {
        Some(Value::Object(ui)) => ui.clone(),
        None => Map::new(),
        Some(_) => return Err(crate::error::invalid("ui metadata must be an object")),
    };
    ui.insert("resourceUri".into(), json!(uri));
    meta.0.insert("ui".into(), Value::Object(ui));
    Ok(())
}

/// Build ordinary rmcp HTML resource contents for an MCP App.
/// Metadata, including `ui.csp` and `openai/ui`, is preserved on these contents.
/// This does not register a resource handler or implement a browser runtime.
pub fn app_html_resource(
    uri: &str,
    html: impl Into<String>,
    meta: MetaObject,
) -> Result<ResourceContents, Error> {
    validate_app_uri(uri)?;
    Ok(ResourceContents::text(html, uri)
        .with_mime_type(APP_MIME_TYPE)
        .with_meta(meta))
}

fn validate_app_uri(uri: &str) -> Result<(), Error> {
    if !uri.starts_with("ui://")
        || fluent_uri::Uri::parse(uri).is_err()
        || uri.len() <= "ui://".len()
    {
        return Err(crate::error::invalid("MCP App resource URI must use ui://"));
    }
    Ok(())
}

/// Display modes supported by an MCP App resource.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    /// Display in the conversation.
    Inline,
    /// Display in the main app area.
    Fullscreen,
    /// Base MCP Apps picture-in-picture mode. Unsupported in OpenAI metadata;
    /// retained for source compatibility, but rejected by `UiResourceMetadata::apply`.
    Pip,
}

/// Display modes a model-triggered tool can prefer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModelDisplayMode {
    /// Display in the conversation.
    Inline,
    /// Display in the main app area.
    Fullscreen,
}

/// The same-server tool invoked by a sidebar shortcut.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum QuickActionTarget {
    /// Invoke this tool with the supplied arguments.
    Tool {
        /// Tool name.
        name: String,
        /// Tool arguments; omission is distinct from an empty object.
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: Option<Map<String, Value>>,
    },
}

/// A sidebar shortcut attached to a global entrypoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QuickAction {
    /// Visible label.
    pub title: String,
    /// At least one icon is required.
    pub icons: Vec<Icon>,
    /// Same-server tool to invoke.
    pub target: QuickActionTarget,
}

/// One host affordance for launching an MCP App.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Entrypoint {
    /// Open files matching the listed dot-prefixed extensions.
    File {
        /// Extensions such as `.stl`.
        extensions: Vec<String>,
    },
    /// Launch from the sidebar.
    Global {
        /// Optional shortcut alongside the app launch action.
        #[serde(rename = "quickAction", skip_serializing_if = "Option::is_none")]
        quick_action: Option<QuickAction>,
    },
    /// Launch the app's settings UI.
    Settings {
        /// Search terms used by the host to discover these settings.
        #[serde(rename = "searchTerms", skip_serializing_if = "Option::is_none")]
        search_terms: Option<Vec<String>>,
    },
    /// Launch in a conversation.
    Thread,
}

/// OpenAI annotations for an MCP App tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiToolMetadata {
    /// Host launch affordances.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoints: Option<Vec<Entrypoint>>,
    /// Preferred display mode when invoked by the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_model_display_mode: Option<ModelDisplayMode>,
}

impl UiToolMetadata {
    /// Validate and insert under `openai/ui`, preserving all other metadata keys.
    /// Does not register the tool or verify same-server tool references.
    pub fn apply(&self, meta: &mut MetaObject) -> Result<(), Error> {
        for entry in self.entrypoints.iter().flatten() {
            match entry {
                Entrypoint::File { extensions } => {
                    if extensions.iter().any(|s| !s.trim().starts_with('.')) {
                        return Err(crate::error::invalid(
                            "file extensions must start with a dot",
                        ));
                    }
                }
                Entrypoint::Global {
                    quick_action: Some(action),
                } => {
                    nonblank(&action.title, "quickAction.title")?;
                    if action.icons.is_empty() {
                        return Err(crate::error::invalid("quickAction.icons must not be empty"));
                    }
                    let QuickActionTarget::Tool { name, .. } = &action.target;
                    nonblank(name, "quickAction.target.name")?;
                }
                Entrypoint::Settings { search_terms } => {
                    for term in search_terms.iter().flatten() {
                        nonblank(term, "searchTerms")?;
                    }
                }
                _ => {}
            }
        }
        let mut value = serde_json::to_value(self)?;
        // Match the normalization used by the other SDKs.
        if let Some(entries) = value.get_mut("entrypoints").and_then(Value::as_array_mut) {
            for entry in entries {
                for key in ["extensions", "searchTerms"] {
                    if let Some(strings) = entry.get_mut(key).and_then(Value::as_array_mut) {
                        for string in strings {
                            if let Some(s) = string.as_str() {
                                *string = Value::String(s.trim().into());
                            }
                        }
                    }
                }
            }
        }
        meta.0.insert(UI_META_KEY.into(), value);
        Ok(())
    }
}

/// OpenAI annotations for an MCP App resource's contents.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiResourceMetadata {
    /// Display modes supported by the app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_display_modes: Option<Vec<DisplayMode>>,
    /// Preferred display mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_display_mode: Option<DisplayMode>,
}

impl UiResourceMetadata {
    /// Validate OpenAI's inline/fullscreen profile and insert under `openai/ui`.
    /// Preserves other metadata keys and leaves them unchanged on error. The host
    /// owns the default modes and may ignore the preferred-mode hint.
    pub fn apply(&self, meta: &mut MetaObject) -> Result<(), Error> {
        if self.preferred_display_mode == Some(DisplayMode::Pip)
            || self
                .available_display_modes
                .iter()
                .flatten()
                .any(|m| *m == DisplayMode::Pip)
        {
            return Err(crate::error::invalid(
                "OpenAI display modes support inline and fullscreen",
            ));
        }
        meta.0
            .insert(UI_META_KEY.into(), serde_json::to_value(self)?);
        Ok(())
    }
}
