//! Validated native settings schemas and layouts. Persistence belongs to the server.
use std::collections::HashSet;

use rmcp::model::{JsonObject, ServerCapabilities, ToolsCapability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Error,
    error::{invalid, nonblank},
    validation::{allowed, compile, validate, validate_required},
};

mod convenience;
mod tools;
mod typed;
mod wire;
pub use tools::SettingsTools;
pub use typed::{SettingsModel, TypedSettings};

/// Capability key identifying a same-server settings read/update pair.
pub const SETTINGS_CAPABILITY_KEY: &str = "openai/settings";

/// Names of the tools the host should invoke for settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsCapability {
    /// Read tool name.
    pub read_tool: String,
    /// Update tool name.
    pub update_tool: String,
}

impl SettingsCapability {
    /// Advertise an installed settings pair in modern and legacy namespaces.
    ///
    /// Call only after both tools are successfully installed and callable. This
    /// description cannot inspect a custom registry or track later mutations;
    /// the consumer must remove/update advertisements when its tools change.
    /// Unrelated namespaces and an existing tools capability are preserved.
    pub fn apply_capabilities(&self, capabilities: &mut ServerCapabilities) {
        capabilities
            .tools
            .get_or_insert_with(ToolsCapability::default);
        let value = serde_json::to_value(self)
            .expect("settings capability contains only strings")
            .as_object()
            .expect("object")
            .clone();
        capabilities
            .extensions
            .get_or_insert_with(Default::default)
            .insert(SETTINGS_CAPABILITY_KEY.into(), value.clone());
        capabilities
            .experimental
            .get_or_insert_with(Default::default)
            .insert(SETTINGS_CAPABILITY_KEY.into(), value);
    }
}

/// A value control or a same-server tool button inside a settings group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum SettingsLayoutItem {
    /// Display this schema property.
    Property {
        /// Exact property name, including unusual JSON names.
        property: String,
    },
    /// Invoke a same-server tool with `{}`. The application must provide that tool.
    Tool {
        /// Tool name.
        tool: String,
        /// Visible label.
        title: String,
        /// Optional description.
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

/// An ordered group of settings controls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum SettingsGroup {
    /// Groups cannot contain other groups.
    Group {
        /// Nonblank section title.
        title: String,
        /// Controls in display order.
        items: Vec<SettingsLayoutItem>,
    },
}

/// Immutable, validated registration for native settings.
/// Fields are primitive boolean/string/number/integer schemas with nonblank titles.
#[derive(Debug)]
pub struct SettingsRegistration {
    pub(crate) capability: SettingsCapability,
    pub(crate) schema: Value,
    pub(crate) layout: Option<Vec<SettingsGroup>>,
    validator: jsonschema::Validator,
    patch_validator: jsonschema::Validator,
}

impl TryFrom<Value> for SettingsRegistration {
    type Error = Error;
    fn try_from(value: Value) -> Result<Self, Error> {
        Self::new(value)
    }
}

impl SettingsRegistration {
    /// Validate a JSON Schema object, using `settings.read` and `settings.update`.
    /// Nested values, schema composition, references and defaults are unsupported.
    pub fn new(schema: Value) -> Result<Self, Error> {
        Self::validate_definition(schema).map_err(Error::configuration)
    }

    fn validate_definition(schema: Value) -> Result<Self, Error> {
        let root = schema
            .as_object()
            .ok_or_else(|| invalid("settings.schema must be an object"))?;
        allowed(
            root,
            &[
                "type",
                "properties",
                "required",
                "$schema",
                "title",
                "description",
                "additionalProperties",
            ],
            "settings.schema",
        )?;
        if root.get("type") != Some(&json!("object")) {
            return Err(invalid("settings.schema.type must be object"));
        }
        if root
            .get("additionalProperties")
            .is_some_and(|v| v != &json!(false))
        {
            return Err(invalid("settings cannot allow additional properties"));
        }
        let fields = root
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("settings.schema.properties must be an object"))?;
        for (name, value) in fields {
            let field = value
                .as_object()
                .ok_or_else(|| invalid(format!("settings.{name} must be an object")))?;
            nonblank(
                field.get("title").and_then(Value::as_str).unwrap_or(""),
                &format!("settings.{name}.title"),
            )?;
            let kind = field.get("type").and_then(Value::as_str).unwrap_or("");
            let mut keywords = vec!["type", "title", "description"];
            match kind {
                "boolean" => {}
                "string" => {
                    keywords.extend(["enum", "minLength", "maxLength", "pattern", "format"])
                }
                "number" | "integer" => keywords.extend([
                    "minimum",
                    "maximum",
                    "exclusiveMinimum",
                    "exclusiveMaximum",
                    "multipleOf",
                ]),
                _ => return Err(invalid(format!("settings.{name} has an unsupported type"))),
            }
            allowed(field, &keywords, &format!("settings.{name}"))?;
            if let Some(options) = field.get("enum")
                && !options
                    .as_array()
                    .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string))
            {
                return Err(invalid(format!(
                    "settings.{name}.enum must contain strings"
                )));
            }
        }
        validate_required(root, fields, "settings")?;
        let complete = wire::values_schema(&schema);
        let validator = compile(&complete)?;
        let patch = wire::patch_schema(&schema);
        let patch_validator = compile(&patch)?;
        Ok(Self {
            capability: SettingsCapability {
                read_tool: "settings.read".into(),
                update_tool: "settings.update".into(),
            },
            schema,
            layout: None,
            validator,
            patch_validator,
        })
    }

    /// Set distinct, nonblank tool names before registration.
    pub fn with_tool_names(
        mut self,
        read: impl Into<String>,
        update: impl Into<String>,
    ) -> Result<Self, Error> {
        let read = read.into();
        let update = update.into();
        nonblank(&read, "readTool").map_err(Error::configuration)?;
        nonblank(&update, "updateTool").map_err(Error::configuration)?;
        if read == update {
            return Err(invalid("settings tool names must be distinct").configuration());
        }
        self.capability = SettingsCapability {
            read_tool: read,
            update_tool: update,
        };
        Ok(self)
    }

    /// Add a layout, rejecting duplicate or unknown property references.
    pub fn with_layout(mut self, layout: Vec<SettingsGroup>) -> Result<Self, Error> {
        let fields = self.schema["properties"]
            .as_object()
            .expect("validated properties");
        let mut seen = HashSet::new();
        for SettingsGroup::Group { title, items } in &layout {
            nonblank(title, "layout.title").map_err(Error::configuration)?;
            for item in items {
                match item {
                    SettingsLayoutItem::Property { property } => {
                        if !fields.contains_key(property) || !seen.insert(property) {
                            return Err(invalid(format!(
                                "unknown or duplicate settings property: {property}"
                            ))
                            .configuration());
                        }
                    }
                    SettingsLayoutItem::Tool { tool, title, .. } => {
                        nonblank(tool, "layout.tool").map_err(Error::configuration)?;
                        nonblank(title, "layout.tool.title").map_err(Error::configuration)?;
                    }
                }
            }
        }
        self.layout = Some(layout);
        Ok(self)
    }

    /// Validate all effective values, including optional fields and server defaults.
    /// Missing/unknown properties and values violating their schemas are rejected.
    pub fn validate_values(&self, values: &JsonObject) -> Result<(), Error> {
        validate(
            &self.validator,
            &Value::Object(values.clone()),
            "settings.values",
        )
    }

    /// Validate a nonempty partial replacement without changing submitted values.
    pub fn validate_patch(&self, set: &JsonObject) -> Result<(), Error> {
        validate(
            &self.patch_validator,
            &Value::Object(set.clone()),
            "settings.set",
        )
    }
}
