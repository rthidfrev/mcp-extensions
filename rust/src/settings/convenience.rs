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

    /// Reference a same-server tool invoked with `{}`. Its declaration supplies
    /// the displayed label. [`super::SettingsRegistration::with_layout`] validates
    /// the name; the application must register and authorize the referenced tool.
    pub fn tool(tool: impl Into<String>) -> Self {
        Self::Tool {
            tool: tool.into(),
            title: None,
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
