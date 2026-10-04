//! OpenAI-specific extensions for MCP servers built with rmcp.
//!
//! Register routes on the application's existing [`rmcp::handler::server::router::tool::ToolRouter`].
//! The application retains its handler, authentication, persistence and transport.
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
mod error;
mod extensions;
pub mod forms;
pub mod mentions;
pub mod resources;
pub mod settings;
pub mod ui;
pub use error::Error;
pub use extensions::OpenAIExtensions;
