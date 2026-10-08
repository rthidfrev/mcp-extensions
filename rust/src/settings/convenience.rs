use super::{SettingsGroup, SettingsLayoutItem};

impl SettingsLayoutItem {
    /// Reference a schema property by its exact JSON name.
    /// [`super::SettingsRegistration::with_layout`] validates that the reference
    /// exists and occurs only once across all groups.
    pub fn property(property: impl Into<String>) -> Self {
        Self::Property {
            property: property.into(),
        }
    }

    /// Describe a same-server tool button invoked with `{}`, without a description.
    /// [`super::SettingsRegistration::with_layout`] validates the tool name and
    /// title. The application must register and authorize the referenced tool.
    pub fn tool(tool: impl Into<String>, title: impl Into<String>) -> Self {
        Self::Tool {
            tool: tool.into(),
            title: title.into(),
            description: None,
        }
    }
}

impl SettingsGroup {
    /// Collect settings controls in display order without registering any tools.
    /// [`super::SettingsRegistration::with_layout`] validates the group title,
    /// tool names and property references when the layout is installed.
    pub fn new(
        title: impl Into<String>,
        items: impl IntoIterator<Item = SettingsLayoutItem>,
    ) -> Self {
        Self::Group {
            title: title.into(),
            items: items.into_iter().collect(),
        }
    }
}
