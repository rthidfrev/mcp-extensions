//! Shared MCP Apps declarations with OpenAI resource annotations.
use mcp_apps_server::{
    MetadataError, ResourceUi, StaticUiResource, UiCsp, UiResource, UiResourceUri, UiUriError,
};
use openai_mcp_extensions::ui::{DisplayMode, UiResourceMetadata};
use rmcp::model::MetaObject;

pub const APP_URI: &str = "ui://parts/viewer";
pub const APP_HTML: &str = include_str!("parts.html");

pub fn resource_ui() -> ResourceUi {
    let mut csp = UiCsp::default();
    csp.connect_domains = Some(vec![]);
    csp.resource_domains = Some(vec![]);
    let mut ui = ResourceUi::default();
    ui.csp = Some(csp);
    ui
}

pub fn resource_meta() -> Result<MetaObject, MetadataError> {
    let mut meta = MetaObject::default();
    UiResourceMetadata {
        available_display_modes: Some(vec![DisplayMode::Inline, DisplayMode::Fullscreen]),
        preferred_display_mode: Some(DisplayMode::Inline),
    }
    .apply(&mut meta)
    .map_err(|error| MetadataError::Preparation(Box::new(error)))?;
    Ok(meta)
}

pub struct PartsView;

impl StaticUiResource for PartsView {
    fn resource() -> Result<UiResource, UiUriError> {
        Ok(UiResource::new(
            UiResourceUri::new(APP_URI)?,
            "parts-viewer",
        ))
    }

    fn html() -> &'static str {
        APP_HTML
    }

    fn ui() -> ResourceUi {
        resource_ui()
    }

    fn meta() -> Result<MetaObject, MetadataError> {
        resource_meta()
    }
}
