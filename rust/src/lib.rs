//! OpenAI-specific extensions for MCP servers built with rmcp.
//!
//! Build standalone routes for a custom dispatcher, or install them on an existing
//! [`rmcp::handler::server::router::tool::ToolRouter`] with [`OpenAIExtensions`].
//! The application retains its handler, authentication, persistence and transport.
//!
//! No optional features are enabled by default. `derive` adds model derives for
//! forms/settings; `mcp-apps` adds generic MCP Apps helpers in [`ui`]. OpenAI
//! entrypoint and display annotations remain available in every configuration.
//! The optional integration uses the published `mcp-apps-server` alpha, pinned to
//! `0.1.0-alpha.1`. Its author's manual host testing covers ChatGPT Desktop only;
//! compatibility with other hosts has not been established.
//!
//! ```
//! use openai_mcp_extensions::{OpenAIExtensions, mentions::MentionSearchResult};
//! use rmcp::handler::server::router::tool::ToolRouter;
//!
//! let mut router: ToolRouter<()> = ToolRouter::new();
//! let mut extensions = OpenAIExtensions::new();
//! extensions.register_mentions(&mut router, |params, context| async move {
//!     // Authorize the caller using context, then search for params.query.
//!     Ok(MentionSearchResult::default())
//! })?;
//! assert!(router.has_route("search_mentions"));
//! # Ok::<(), openai_mcp_extensions::Error>(())
//! ```
#![cfg_attr(feature = "derive", doc = include_str!("model/derive_contracts.md"))]

pub mod client;
mod error;
mod extensions;
pub mod forms;
pub mod mentions;
pub mod model;
pub mod resources;
pub mod settings;
pub mod ui;
mod validation;
pub use client::OpenAIRequestContextExt;
pub use error::Error;
pub use extensions::{ExtensionFuture, OpenAIExtensions};
pub use model::{FieldType, Model, ModelField};
pub use settings::SettingsModel;
pub use ui::{OpenAIUiResourceExt, OpenAIUiToolExt};

#[cfg(feature = "derive")]
pub use openai_mcp_extensions_macros::{Model, SettingsModel, StringEnum};

/// Implementation dependencies used by the optional derives.
#[doc(hidden)]
pub mod __private {
    pub use serde;
    pub use serde_json;
}
