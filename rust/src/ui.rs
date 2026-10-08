//! OpenAI entrypoints and display annotations for native rmcp tools and resources.
//!
//! The `openai/ui` profile is available without optional features. Your server
//! owns generic MCP Apps metadata, resource serving and request authorization.
//! Enable `mcp-apps` for checked resource associations, HTML contents and client
//! capability inspection backed by MCP Apps Server. These helpers do not change
//! the OpenAI annotations or require adopting its router.
#[cfg(feature = "mcp-apps")]
mod mcp_apps;
#[cfg(feature = "mcp-apps")]
pub use mcp_apps::{
    APP_CAPABILITY_KEY, APP_MIME_TYPE, app_html_resource, associate_app_resource, supports_mcp_apps,
};

use rmcp::model::{Icon, MetaObject, ResourceContents, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Error, error::nonblank};

/// `_meta` key for OpenAI UI annotations.
pub const UI_META_KEY: &str = "openai/ui";

/// Display modes supported by OpenAI tool and resource UI annotations.
/// Unsupported modes such as `pip` are rejected during deserialization.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    /// Display in the conversation.
    Inline,
    /// Display in the main app area.
    Fullscreen,
}

/// Display modes a model-triggered tool can prefer, using the same OpenAI profile.
pub type ModelDisplayMode = DisplayMode;

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
    /// Invalid definitions leave the original metadata unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] for invalid file extensions, blank labels
    /// or names, missing quick-action icons, or metadata serialization failures.
    pub fn apply(&self, meta: &mut MetaObject) -> Result<(), Error> {
        for entry in self.entrypoints.iter().flatten() {
            match entry {
                Entrypoint::File { extensions } => {
                    if extensions.iter().any(|s| !s.trim().starts_with('.')) {
                        return Err(
                            crate::error::invalid("file extensions must start with a dot")
                                .configuration(),
                        );
                    }
                }
                Entrypoint::Global {
                    quick_action: Some(action),
                } => {
                    nonblank(&action.title, "quickAction.title").map_err(Error::configuration)?;
                    if action.icons.is_empty() {
                        return Err(crate::error::invalid("quickAction.icons must not be empty")
                            .configuration());
                    }
                    let QuickActionTarget::Tool { name, .. } = &action.target;
                    nonblank(name, "quickAction.target.name").map_err(Error::configuration)?;
                }
                Entrypoint::Settings { search_terms } => {
                    for term in search_terms.iter().flatten() {
                        nonblank(term, "searchTerms").map_err(Error::configuration)?;
                    }
                }
                _ => {}
            }
        }
        let mut value = serde_json::to_value(self).map_err(|e| Error::from(e).configuration())?;
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
    /// OpenAI display modes supported by the app: inline and fullscreen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_display_modes: Option<Vec<DisplayMode>>,
    /// Preferred OpenAI display mode: inline or fullscreen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_display_mode: Option<DisplayMode>,
}

impl UiResourceMetadata {
    /// Insert OpenAI's inline/fullscreen annotations under `openai/ui`.
    /// Preserves other metadata keys and leaves them unchanged on error. The host
    /// owns the default modes and may ignore the preferred-mode hint.
    /// Unsupported wire modes are rejected when deserializing this type.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] if the annotations cannot be serialized.
    pub fn apply(&self, meta: &mut MetaObject) -> Result<(), Error> {
        let value = serde_json::to_value(self).map_err(|e| Error::from(e).configuration())?;
        meta.0.insert(UI_META_KEY.into(), value);
        Ok(())
    }
}

/// Apply UI annotations directly to an existing native rmcp tool description.
/// These methods only change metadata; the server still owns tool registration
/// and resource serving.
pub trait OpenAIUiToolExt {
    /// Associate the tool with an existing `ui://` resource using
    /// [`associate_app_resource`]. Preserves other metadata and tool fields.
    /// Invalid URIs or malformed `ui` metadata leave the entire tool unchanged.
    /// Conflicting existing resource associations also fail without mutation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] as described by [`associate_app_resource`].
    /// Available with the `mcp-apps` feature.
    #[cfg(feature = "mcp-apps")]
    fn associate_app_resource(&mut self, uri: &str) -> Result<(), Error>;

    /// Replace `openai/ui` using [`UiToolMetadata::apply`], preserving other
    /// metadata and tool fields. Invalid annotations leave the tool unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] as described by [`UiToolMetadata::apply`].
    fn apply_openai_ui(&mut self, metadata: &UiToolMetadata) -> Result<(), Error>;
}

impl OpenAIUiToolExt for Tool {
    #[cfg(feature = "mcp-apps")]
    fn associate_app_resource(&mut self, uri: &str) -> Result<(), Error> {
        let mut meta = self.meta.clone().unwrap_or_default();
        associate_app_resource(&mut meta, uri)?;
        self.meta = Some(meta);
        Ok(())
    }

    fn apply_openai_ui(&mut self, metadata: &UiToolMetadata) -> Result<(), Error> {
        let mut meta = self.meta.clone().unwrap_or_default();
        metadata.apply(&mut meta)?;
        self.meta = Some(meta);
        Ok(())
    }
}

/// Apply OpenAI display annotations to existing native rmcp resource contents.
pub trait OpenAIUiResourceExt {
    /// Replace `openai/ui` using [`UiResourceMetadata::apply`]. Preserves URI,
    /// MIME type, text/blob contents and every other metadata namespace.
    /// Invalid annotations or an unsupported rmcp content variant leave the
    /// entire resource content unchanged; this does not register or serve it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] for serialization failures or an
    /// unsupported rmcp resource content variant.
    fn apply_openai_ui(&mut self, metadata: &UiResourceMetadata) -> Result<(), Error>;
}

impl OpenAIUiResourceExt for ResourceContents {
    fn apply_openai_ui(&mut self, metadata: &UiResourceMetadata) -> Result<(), Error> {
        let original = match self {
            ResourceContents::TextResourceContents { meta, .. }
            | ResourceContents::BlobResourceContents { meta, .. } => meta,
            _ => {
                return Err(
                    crate::error::invalid("unsupported rmcp resource content variant")
                        .configuration(),
                );
            }
        };
        let mut meta = original.clone().unwrap_or_default();
        metadata.apply(&mut meta)?;
        *original = Some(meta);
        Ok(())
    }
}
