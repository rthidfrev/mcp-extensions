//! Composer mention search, exposed as an ordinary read-only MCP tool.
use rmcp::model::{Icon, Resource};
use serde::{Deserialize, Serialize};

use crate::{Error, error::nonblank};

/// Name of the mention-search tool installed by the SDK.
pub const SEARCH_MENTIONS_TOOL: &str = "search_mentions";

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
