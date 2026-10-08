//! OpenAI form field, choice and resource-selection rules.
use super::{PREVIEW_META_KEY, PreviewTarget, ResourceInput, ResourceSelection};
use crate::{
    Error,
    error::{invalid, nonblank},
    validation::{allowed, compile, validate},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

pub(super) fn validate_field(value: &Value) -> Result<jsonschema::Validator, Error> {
    let field = value
        .as_object()
        .ok_or_else(|| invalid("field must be an object"))?;
    let kind = field.get("type").and_then(Value::as_str).unwrap_or("");
    let mut keys = vec!["type", "title", "description", "default"];
    match kind {
        "string" => keys.extend([
            "minLength",
            "maxLength",
            "pattern",
            "format",
            "enum",
            "enumNames",
            "oneOf",
            "x-openai-suggestions",
            "x-openai-input",
        ]),
        "number" | "integer" => keys.extend([
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "multipleOf",
        ]),
        "boolean" => {}
        "array" => keys.extend([
            "items",
            "minItems",
            "maxItems",
            "uniqueItems",
            "x-openai-input",
        ]),
        _ => return Err(invalid("unsupported field type")),
    }
    allowed(field, &keys, "field")?;
    for key in ["title", "description"] {
        if field.get(key).is_some_and(|v| !v.is_string()) {
            return Err(invalid(format!("{key} must be a string")));
        }
    }
    if let Some(options) = field.get("enum") {
        string_enum(options)?;
    }
    if let Some(names) = field.get("enumNames") {
        string_enum(names)?;
        if field.get("enum").and_then(Value::as_array).map(Vec::len)
            != names.as_array().map(Vec::len)
        {
            return Err(invalid("enumNames must correspond to enum values"));
        }
    }
    if let Some(options) = field.get("oneOf") {
        validate_options(options)?;
    }
    if let Some(options) = field.get("x-openai-suggestions") {
        validate_options(options)?;
    }
    if field.contains_key("oneOf") && field.contains_key("enum") {
        return Err(invalid("field cannot combine enum and oneOf"));
    }
    if let Some(format) = field.get("format")
        && !format
            .as_str()
            .is_some_and(|s| ["email", "uri", "date", "date-time"].contains(&s))
    {
        return Err(invalid("unsupported string format"));
    }
    if kind == "array" {
        let items = field
            .get("items")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("array.items must be an object"))?;
        if let Some(options) = items.get("anyOf") {
            allowed(items, &["anyOf"], "items")?;
            validate_options(options)?;
        } else {
            if items.get("type") != Some(&json!("string")) {
                return Err(invalid("array items must be strings"));
            }
            validate_field(&Value::Object(items.clone()))?;
            if ["oneOf", "x-openai-input"]
                .iter()
                .any(|k| items.contains_key(*k))
            {
                return Err(invalid("unsupported array item input"));
            }
        }
    }
    if let Some(input) = field.get("x-openai-input") {
        let input: ResourceInput = serde_json::from_value(input.clone())
            .map_err(|e| invalid(format!("x-openai-input: {e}")))?;
        let uri_schema = if kind == "array" {
            &field["items"]
        } else {
            value
        };
        if uri_schema.get("type") != Some(&json!("string"))
            || uri_schema.get("format") != Some(&json!("uri"))
            || ["enum", "oneOf", "anyOf"]
                .iter()
                .any(|k| uri_schema.get(*k).is_some())
        {
            return Err(invalid(
                "resource input requires URI strings without enum constraints",
            ));
        }
        if kind == "string" && input.selection.is_some() {
            return Err(invalid("single resource input cannot specify selection"));
        }
        for resource in &input.options {
            validate_uri(&resource.uri, "resource.uri")?;
            nonblank(&resource.name, "resource.name")?;
            if let Some(thumbnail) = resource
                .meta
                .as_ref()
                .and_then(|m| m.0.get("openai/thumbnail"))
            {
                // The titled-choice source restriction is separate from the
                // resource metadata's delegated MCP.Icon shape.
                let _: rmcp::model::Icon = serde_json::from_value(thumbnail.clone())
                    .map_err(|e| invalid(format!("openai/thumbnail: {e}")))?;
            }
            if let Some(preview) = resource
                .meta
                .as_ref()
                .and_then(|m| m.0.get(PREVIEW_META_KEY))
            {
                let preview: super::wire::PreviewMetadata<PreviewTarget> =
                    serde_json::from_value(preview.clone())
                        .map_err(|e| invalid(format!("openai/preview: {e}")))?;
                preview.target.validate()?;
            }
        }
        if let Some(default) = field.get("default") {
            if input.selection == Some(ResourceSelection::Implicit) {
                return Err(invalid("implicit selection cannot specify a default"));
            }
            for uri in selected(default)? {
                if !input.options.iter().any(|r| r.uri == uri) {
                    return Err(invalid("resource defaults must name supplied options"));
                }
            }
        }
    }
    let validator = compile(value)?;
    if let Some(default) = field.get("default") {
        validate(&validator, default, "field.default")?;
    }
    Ok(validator)
}

fn string_enum(value: &Value) -> Result<(), Error> {
    if !value
        .as_array()
        .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string))
    {
        return Err(invalid("enum values must be a nonempty string array"));
    }
    Ok(())
}
fn validate_options(value: &Value) -> Result<(), Error> {
    let options = value
        .as_array()
        .ok_or_else(|| invalid("options must be an array"))?;
    for option in options {
        let option = option
            .as_object()
            .ok_or_else(|| invalid("option must be an object"))?;
        allowed(
            option,
            &[
                "const",
                "title",
                "description",
                "x-openai-thumbnail",
                "x-openai-preview",
            ],
            "option",
        )?;
        if !option.get("const").is_some_and(Value::is_string) {
            return Err(invalid("option.const must be a string"));
        }
        if !option.get("title").is_some_and(Value::is_string) {
            return Err(invalid("option.title must be a string"));
        }
        if option.get("description").is_some_and(|v| !v.is_string()) {
            return Err(invalid("option.description must be a string"));
        }
        for key in ["x-openai-thumbnail", "x-openai-preview"] {
            if let Some(icon) = option.get(key) {
                let icon: rmcp::model::Icon = serde_json::from_value(icon.clone())?;
                validate_thumbnail_source(&icon.src)?;
            }
        }
    }
    Ok(())
}
pub(super) fn validate_uri(uri: &str, location: &str) -> Result<(), Error> {
    nonblank(uri, location)?;
    // Validate an absolute URI without interpreting it as a filesystem path.
    if fluent_uri::Uri::parse(uri).is_err() {
        return Err(invalid(format!("{location} must be an absolute URI")));
    }
    Ok(())
}

fn validate_thumbnail_source(src: &str) -> Result<(), Error> {
    if let Ok(uri) = fluent_uri::Uri::parse(src)
        && uri.scheme().as_str().eq_ignore_ascii_case("https")
        && uri.authority().is_some_and(|a| !a.host().is_empty())
    {
        return Ok(());
    }
    if src
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
        && fluent_uri::Uri::parse(src).is_ok()
        && let Some((header, encoded)) = src[5..].split_once(',')
        && let Some(header) = header.strip_suffix(";base64")
        && let Some(media_type) = header.split(';').next()
        && media_type
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
        && let Some(subtype) = media_type.get(6..)
        && !subtype.is_empty()
        && subtype
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
        && STANDARD
            .decode(encoded)
            .is_ok_and(|bytes| !bytes.is_empty())
    {
        return Ok(());
    }
    Err(invalid(
        "thumbnail.src must be an HTTPS URL or a base64 image data URI",
    ))
}
pub(crate) fn validate_resource_selection(
    input: &Value,
    value: &Value,
    location: &str,
) -> Result<(), Error> {
    let input: ResourceInput = serde_json::from_value(input.clone())?;
    for uri in selected(value)? {
        if !input.permits(uri) {
            return Err(invalid(format!(
                "{location}: resource is not a supplied option"
            )));
        }
    }
    Ok(())
}

fn selected(value: &Value) -> Result<Vec<&str>, Error> {
    if let Some(uri) = value.as_str() {
        return Ok(vec![uri]);
    }
    value
        .as_array()
        .ok_or_else(|| invalid("resource selection must be a string or string array"))?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| invalid("resource selection must contain strings"))
        })
        .collect()
}
