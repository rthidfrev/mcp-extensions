//! Shared schema checks and value diagnostics for OpenAI protocol subsets.
use crate::{Error, error::invalid};
use rmcp::model::JsonObject;
use serde_json::Value;
use std::collections::HashSet;

pub(crate) fn pointer(name: &str) -> String {
    format!("/{}", name.replace('~', "~0").replace('/', "~1"))
}

pub(crate) fn failure(path: impl Into<String>, message: impl Into<String>) -> Error {
    Error::ModelValidation {
        path: path.into(),
        message: message.into(),
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
        .map_err(|error| {
            failure(
                error.instance_path().to_string(),
                "invalid JSON Schema definition",
            )
        })
}

pub(crate) fn validate(
    validator: &jsonschema::Validator,
    value: &Value,
    location: &str,
) -> Result<(), Error> {
    validator
        .validate(value)
        .map_err(|error| Error::ModelValidation {
            path: error.instance_path().to_string(),
            // JSON Schema's formatted error can include the submitted value.
            message: format!("{location}: value violates the schema"),
        })
}
