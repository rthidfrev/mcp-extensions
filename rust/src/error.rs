use rmcp::{ErrorData, service::ServiceError};

/// Local configuration, schema or peer-response failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A value violates an extension contract. The message includes its location.
    #[error("{0}")]
    Validation(String),
    /// Registration would overwrite an existing rmcp route.
    #[error("tool already registered: {0}")]
    ToolCollision(String),
    /// The peer did not advertise the required extension capability.
    #[error("the MCP client does not support openai/elicitation form requests")]
    UnsupportedClient,
    /// Server-initiated requests are unavailable on the modern MCP lifecycle.
    #[error(
        "direct form elicitation requires a legacy MCP connection; use MRTR for modern requests"
    )]
    UnsupportedProtocol,
    /// The MCP request failed, timed out or was cancelled.
    #[error(transparent)]
    Transport(#[from] ServiceError),
    /// A peer response could not be decoded.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<Error> for ErrorData {
    fn from(error: Error) -> Self {
        Self::invalid_params(error.to_string(), None)
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Validation(message.into())
}

pub(crate) fn nonblank(value: &str, location: &str) -> Result<(), Error> {
    if value.trim().is_empty() {
        return Err(invalid(format!("{location} must not be blank")));
    }
    Ok(())
}
