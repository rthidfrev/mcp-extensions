//! Composer mention search, exposed as an ordinary read-only MCP tool.
use rmcp::model::{Icon, Resource, ServerCapabilities, ToolsCapability};
use serde::{Deserialize, Serialize};

use crate::{Error, error::nonblank};

mod tool;
pub use tool::{search_tool, search_tool_with_state};

/// Name of the mention-search tool installed by the SDK.
pub const SEARCH_MENTIONS_TOOL: &str = "search_mentions";

/// Capability key identifying the mention-search tool.
pub const MENTIONS_CAPABILITY_KEY: &str = "openai/mentions";

/// Advertise the SDK's mention tool after installing it in your registry.
///
/// Modern and legacy capability maps preserve unrelated namespaces and an existing
/// tools capability. Keep this advertisement consistent with the live registry
/// if the route is later disabled or removed.
pub fn apply_capabilities(capabilities: &mut ServerCapabilities) {
    capabilities
        .tools
        .get_or_insert_with(ToolsCapability::default);
    let value = serde_json::json!({"searchTool": SEARCH_MENTIONS_TOOL})
        .as_object()
        .expect("object")
        .clone();
    capabilities
        .extensions
        .get_or_insert_with(Default::default)
        .insert(MENTIONS_CAPABILITY_KEY.into(), value.clone());
    capabilities
        .experimental
        .get_or_insert_with(Default::default)
        .insert(MENTIONS_CAPABILITY_KEY.into(), value);
}

/// Mention search input. An empty query is valid.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MentionSearchParams {
    /// Search text provided by the host.
    pub query: String,
}

/// The OpenAI resource presentation accepted by the existing SDKs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MentionResource {
    /// Nonblank URI.
    #[serde(rename = "resourceUri")]
    pub resource_uri: String,
    /// Nonblank display title.
    pub title: String,
    /// Optional nonblank secondary label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Resource icons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
}

/// Native MCP resource links or the resource presentation used by the other SDKs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MentionItem {
    /// Standard MCP `resource_link`.
    ResourceLink(Resource),
    /// OpenAI `resource` presentation.
    Resource(MentionResource),
}

/// Mention-search structured content.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MentionSearchResult {
    /// Search matches, in display order.
    pub items: Vec<MentionItem>,
}

impl MentionSearchResult {
    /// Validate presentation fields before sending a handler result to the host.
    pub fn validate(&self) -> Result<(), Error> {
        for item in &self.items {
            match item {
                MentionItem::ResourceLink(link) => {
                    nonblank(&link.uri, "mention.uri")?;
                    nonblank(&link.name, "mention.name")?;
                }
                MentionItem::Resource(MentionResource {
                    resource_uri,
                    title,
                    subtitle,
                    ..
                }) => {
                    nonblank(resource_uri, "mention.resourceUri")?;
                    nonblank(title, "mention.title")?;
                    if let Some(subtitle) = subtitle {
                        nonblank(subtitle, "mention.subtitle")?;
                    }
                }
            }
        }
        Ok(())
    }
}
