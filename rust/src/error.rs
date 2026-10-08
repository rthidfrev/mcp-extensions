use rmcp::{ErrorData, service::ServiceError};
use serde_json::{Value, json};

/// Configuration, request-metadata, schema or peer-response failures.
/// Match the variants needed by your application and retain a fallback arm;
/// optional integrations can add variants through Cargo feature unification.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A value violates an extension contract. The message includes its location.
    #[error("{0}")]
    Validation(String),
    /// Registration conflicts with an existing route or a reserved disabled name.
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
    /// An input JSON value could not be decoded. Inspect the source locally;
    /// the display message deliberately omits the submitted value.
    #[error("JSON does not match the expected extension value")]
    Json(#[from] serde_json::Error),
    /// A generic MCP Apps resource identifier is invalid.
    /// Available with the `mcp-apps` feature.
    #[cfg(feature = "mcp-apps")]
    #[error("{0}")]
    McpAppsUri(#[from] mcp_apps_server::UiUriError),
    /// Generic MCP Apps metadata is malformed or conflicts with existing data.
    /// Available with the `mcp-apps` feature.
    #[cfg(feature = "mcp-apps")]
    #[error("{0}")]
    McpAppsMetadata(#[from] mcp_apps_server::MetadataError),
    /// An application-supplied extension definition is invalid.
    #[error("invalid extension configuration: {source}")]
    Configuration {
        /// Original definition error, including any structured location.
        #[source]
        source: Box<Error>,
    },
    /// A response to a server-initiated request violates the extension contract.
    #[error("invalid extension response: {source}")]
    InvalidResponse {
        /// Original response error, including any structured location.
        #[source]
        source: Box<Error>,
    },
    /// A typed model violates a contract at the given JSON Pointer.
    #[error("{path}: {message}")]
    ModelValidation {
        /// JSON Pointer relative to the model, schema or request metadata.
        path: String,
        /// Explanation without submitted values.
        message: String,
    },
    /// A validated wire value cannot be represented by its Rust model.
    #[error("{path}: value cannot be represented by its Rust model")]
    ModelConversion {
        /// JSON Pointer relative to the model; empty means the complete model.
        path: String,
        /// Original Serde conversion error.
        #[source]
        source: serde_json::Error,
    },
}

impl Error {
    /// Inspect a structured validation/conversion failure's JSON Pointer without
    /// parsing its display message. Contextual errors retain the original path.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::ModelValidation { path, .. } | Self::ModelConversion { path, .. } => Some(path),
            Self::Configuration { source } | Self::InvalidResponse { source } => source.path(),
            _ => None,
        }
    }

    pub(crate) fn configuration(self) -> Self {
        match self {
            Self::Configuration { .. } => self,
            source => Self::Configuration {
                source: Box::new(source),
            },
        }
    }

    pub(crate) fn invalid_response(self) -> Self {
        match self {
            Self::InvalidResponse { .. } => self,
            source => Self::InvalidResponse {
                source: Box::new(source),
            },
        }
    }

    fn diagnostic_data(&self) -> Option<Value> {
        self.path().map(|path| json!({"path": path}))
    }

    pub(crate) fn into_internal(self) -> ErrorData {
        ErrorData::internal_error(self.to_string(), self.diagnostic_data())
    }
}

/// Classify errors at the JSON-RPC boundary. Input validation/decoding maps to
/// invalid parameters, local configuration and invalid peer responses to internal
/// errors, and an unavailable lifecycle to invalid request. Missing capability
/// uses MCP's dedicated error; peer errors retain their code, message and data.
/// Local transport failures are internal errors, without transport details or
/// cancellation reasons that could contain application data.
impl From<Error> for ErrorData {
    fn from(error: Error) -> Self {
        match error {
            Error::Transport(ServiceError::McpError(peer)) => peer,
            Error::Transport(source) => {
                let message = match source {
                    ServiceError::TransportSend(_) => "MCP transport failed to send the request",
                    ServiceError::TransportClosed => "MCP transport closed",
                    ServiceError::UnexpectedResponse => "MCP peer returned an unexpected response",
                    ServiceError::SubscriptionLagged { .. } => "MCP subscription consumer lagged",
                    ServiceError::Cancelled { .. } => "MCP request was cancelled",
                    ServiceError::Timeout { .. } => "MCP request timed out",
                    ServiceError::InputRequiredRoundsExceeded { .. } => {
                        "MCP input-required round limit exceeded"
                    }
                    ServiceError::McpError(_) => unreachable!("peer error handled above"),
                    _ => "MCP service request failed",
                };
                Self::internal_error(message, None)
            }
            Error::UnsupportedClient => {
                Self::missing_required_client_capability(crate::forms::required_capabilities())
            }
            Error::UnsupportedProtocol => Self::invalid_request(error.to_string(), None),
            Error::ToolCollision(_)
            | Error::Configuration { .. }
            | Error::InvalidResponse { .. } => error.into_internal(),
            #[cfg(feature = "mcp-apps")]
            Error::McpAppsUri(_) | Error::McpAppsMetadata(_) => error.into_internal(),
            Error::Validation(_)
            | Error::Json(_)
            | Error::ModelValidation { .. }
            | Error::ModelConversion { .. } => {
                Self::invalid_params(error.to_string(), error.diagnostic_data())
            }
        }
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
