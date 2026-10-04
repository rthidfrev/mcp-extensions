//! Inputs supplied by the host when it opens a file in an MCP App.
use rmcp::model::RequestMetaObject;
use serde::{Deserialize, Serialize};

use crate::{Error, error::nonblank};

/// Metadata key carrying the host-owned resource path.
pub const RESOURCE_META_KEY: &str = "openai/resource";

/// Representation requested for an ordinary server-owned resource.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceRepresentation {
    /// UTF-8 text in MCP `TextResourceContents`.
    Text,
    /// Base64 bytes in MCP `BlobResourceContents`.
    Blob,
}

/// Decode the optional representation hint on `resources/read`.
/// Server-owned resources may honor it; file-entrypoint reads are handled by the
/// host. Missing hints return `None`. Invalid metadata returns an error.
/// Other metadata is ignored and remains available on the original request.
pub fn requested_representation(
    meta: &RequestMetaObject,
) -> Result<Option<ResourceRepresentation>, Error> {
    let Some(resource) = meta.0.get(RESOURCE_META_KEY) else {
        return Ok(None);
    };
    let resource = resource
        .as_object()
        .ok_or_else(|| crate::error::invalid("openai/resource must be an object"))?;
    resource
        .get("representation")
        .map(|value| serde_json::from_value(value.clone()).map_err(Error::from))
        .transpose()
}

/// A file selected by the host for an app launch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntrypointFile {
    /// Display name supplied by the host.
    pub name: String,
    /// Resource URI; it is not a server filesystem path.
    pub resource_uri: String,
}

/// Arguments passed to a file-entrypoint tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntrypointInput {
    /// File being opened.
    pub file: FileEntrypointFile,
}

impl FileEntrypointInput {
    /// Validate required, nonblank file fields without reading the file.
    pub fn validate(&self) -> Result<(), Error> {
        nonblank(&self.file.name, "file.name")?;
        nonblank(&self.file.resource_uri, "file.resourceUri")
    }
}

/// The host's resource metadata on a tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceToolCallMetadata {
    /// A path in the host's filesystem, not necessarily accessible to the server.
    pub path: String,
}

/// Read `openai/resource` from tool-call metadata. Missing metadata returns `None`;
/// malformed metadata returns an error. This helper never accesses the filesystem.
pub fn resource_tool_call_metadata(
    meta: &RequestMetaObject,
) -> Result<Option<ResourceToolCallMetadata>, Error> {
    meta.0
        .get(RESOURCE_META_KEY)
        .map(|value| serde_json::from_value(value.clone()).map_err(Error::from))
        .transpose()
}
