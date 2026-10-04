//! Validated native settings schemas and layouts. Persistence belongs to the server.
use std::collections::HashSet;

use rmcp::model::JsonObject;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Error,
    error::{invalid, nonblank},
};

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

impl SettingsRegistration {
    /// Validate a JSON Schema object, using `settings.read` and `settings.update`.
    /// Nested values, schema composition, references and defaults are unsupported.
    pub fn new(schema: Value) -> Result<Self, Error> {
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
        let mut complete = schema.clone();
        complete["required"] = json!(fields.keys().collect::<Vec<_>>());
        complete["additionalProperties"] = json!(false);
        let validator = compile(&complete)?;
        let mut patch = complete;
        patch["required"] = json!([]);
        patch["minProperties"] = json!(1);
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
        nonblank(&read, "readTool")?;
        nonblank(&update, "updateTool")?;
        if read == update {
            return Err(invalid("settings tool names must be distinct"));
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
            nonblank(title, "layout.title")?;
            for item in items {
                match item {
                    SettingsLayoutItem::Property { property } => {
                        if !fields.contains_key(property) || !seen.insert(property) {
                            return Err(invalid(format!(
                                "unknown or duplicate settings property: {property}"
                            )));
                        }
                    }
                    SettingsLayoutItem::Tool { tool, title, .. } => {
                        nonblank(tool, "layout.tool")?;
                        nonblank(title, "layout.tool.title")?;
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

pub(crate) fn allowed(object: &JsonObject, keys: &[&str], location: &str) -> Result<(), Error> {
    if let Some(key) = object.keys().find(|k| !keys.contains(&k.as_str())) {
        return Err(invalid(format!("unsupported keyword {location}.{key}")));
    }
    Ok(())
}

pub(crate) fn validate_required(
    root: &JsonObject,
    fields: &JsonObject,
    location: &str,
) -> Result<(), Error> {
    if let Some(required) = root.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| invalid(format!("{location}.required must be an array")))?;
        let mut seen = HashSet::new();
        for name in required {
            let name = name
                .as_str()
                .ok_or_else(|| invalid("required names must be strings"))?;
            if !fields.contains_key(name) || !seen.insert(name) {
                return Err(invalid(format!(
                    "unknown or duplicate required property: {name}"
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn compile(schema: &Value) -> Result<jsonschema::Validator, Error> {
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(true)
        .build(schema)
        .map_err(|e| invalid(format!("invalid schema: {e}")))
}

pub(crate) fn validate(
    validator: &jsonschema::Validator,
    value: &Value,
    location: &str,
) -> Result<(), Error> {
    validator
        .validate(value)
        .map_err(|e| invalid(format!("{location}{}: {e}", e.instance_path())))
}
