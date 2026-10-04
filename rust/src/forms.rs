//! Lossless extended form schemas, direct elicitation and modern tool round trips.
//!
//! rmcp's standard `ElicitationSchema` cannot represent all OpenAI fields. This
//! module keeps the wire schema as JSON instead of converting through that type.
use base64::{Engine, engine::general_purpose::STANDARD};
use rmcp::{
    RoleServer,
    model::{ClientCapabilities, CustomRequest, Resource, ServerRequest},
    service::{PeerRequestOptions, RequestContext},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub mod mrtr;

use crate::{
    Error,
    error::{invalid, nonblank},
    settings::{allowed, compile, validate, validate_required},
};

/// Extension capability used by an OpenAI form host.
pub const ELICITATION_CAPABILITY_KEY: &str = "openai/elicitation";
/// Custom RPC method for direct OpenAI form requests.
pub const ELICITATION_METHOD: &str = "openai/elicitation/create";

/// Metadata key for expanded previews on resource options.
pub const PREVIEW_META_KEY: &str = "openai/preview";

/// A same-server tool or MCP resource opened by the host as a resource preview.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PreviewTarget {
    /// Invoke an MCP App tool on the originating server.
    #[serde(rename = "mcp_app_tool")]
    McpAppTool {
        /// Nonblank tool name. The application must register this tool.
        name: String,
        /// Omission has the host-defined meaning of `{}`.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "preview_arguments"
        )]
        arguments: Option<serde_json::Map<String, Value>>,
    },
    /// Read this linked resource using ordinary MCP resource methods.
    #[serde(rename = "resource_link")]
    ResourceLink {
        /// Native MCP resource fields, including icons and metadata.
        #[serde(flatten)]
        resource: Resource,
    },
}

fn preview_arguments<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<serde_json::Map<String, Value>>, D::Error> {
    // Omission is valid; an explicitly supplied value must be an object.
    serde_json::Map::deserialize(deserializer).map(Some)
}

impl PreviewTarget {
    /// Validate the target and insert its metadata, preserving other namespaces.
    /// This does not resolve the resource, register the tool or open the preview.
    pub fn apply(&self, meta: &mut rmcp::model::MetaObject) -> Result<(), Error> {
        self.validate()?;
        meta.0
            .insert(PREVIEW_META_KEY.into(), json!({"target":self}));
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        match self {
            Self::McpAppTool { name, .. } => nonblank(name, "preview.target.name"),
            Self::ResourceLink { resource } => {
                nonblank(&resource.name, "preview.target.name")?;
                validate_uri(&resource.uri, "preview.target.uri")
            }
        }
    }
}

/// User-selectable resource source.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    /// Select files.
    File,
    /// Select directories.
    Directory,
}

/// Host file-picker configuration. It permits selecting resources outside options.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UserResourceOptions {
    /// Kind of resource the picker accepts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ResourceKind>,
    /// File accept filters interpreted by the host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept: Option<Vec<String>>,
}

/// Selection behavior of a multiple-resource picker.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceSelection {
    /// Require selections to name supplied options, unless user options are enabled.
    Explicit,
    /// Let the host determine selected resources. A default is not allowed.
    Implicit,
}

/// Canonical resource input or the legacy `file` spelling accepted by existing SDKs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceInputType {
    /// Canonical resource selector.
    Resource,
    /// Legacy alias.
    File,
}

/// OpenAI resource-selection annotations for a string URI or an array of URI strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceInput {
    /// Use `Resource` for new forms.
    #[serde(rename = "type")]
    pub input_type: ResourceInputType,
    /// Resources displayed as choices; their metadata can carry previews/thumbnails.
    pub options: Vec<Resource>,
    /// Optional host picker allowing resources outside `options`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_options: Option<UserResourceOptions>,
    /// Only valid on a multiple-resource field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<ResourceSelection>,
}

impl ResourceInput {
    /// Canonical explicit resource input without a user picker.
    pub fn new(options: Vec<Resource>) -> Self {
        Self {
            input_type: ResourceInputType::Resource,
            options,
            user_options: None,
            selection: None,
        }
    }
    fn permits(&self, value: &str) -> bool {
        self.selection == Some(ResourceSelection::Implicit)
            || self.user_options.is_some()
            || self.options.iter().any(|r| r.uri == value)
    }
}

/// A validated field from the supported flat OpenAI form schema subset.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct FormField(Value);

impl FormField {
    /// Validate a raw field without stripping OpenAI extensions or constraints.
    /// Supports primitives, enums, string arrays, suggestions and resource selectors.
    /// Unknown keywords, nested objects, references and unsupported inputs are rejected.
    pub fn from_json(value: Value) -> Result<Self, Error> {
        validate_field(&value)?;
        Ok(Self(value))
    }
    /// Build a plain string field with optional length/pattern annotations added via JSON.
    pub fn string(title: impl Into<String>) -> Self {
        Self(json!({"type":"string","title":title.into()}))
    }
    /// Build a boolean field.
    pub fn boolean(title: impl Into<String>) -> Self {
        Self(json!({"type":"boolean","title":title.into()}))
    }
    /// Build a numeric field.
    pub fn number(title: impl Into<String>) -> Self {
        Self(json!({"type":"number","title":title.into()}))
    }
    /// Build an integer field.
    pub fn integer(title: impl Into<String>) -> Self {
        Self(json!({"type":"integer","title":title.into()}))
    }
    /// Build a resource picker, preserving native MCP resource metadata.
    /// Multiple selection returns an array; single selection returns one URI.
    pub fn resource(
        title: impl Into<String>,
        input: ResourceInput,
        multiple: bool,
    ) -> Result<Self, Error> {
        let mut value = if multiple {
            json!({"type":"array","items":{"type":"string","format":"uri"}})
        } else {
            json!({"type":"string","format":"uri"})
        };
        value["title"] = json!(title.into());
        value["x-openai-input"] = serde_json::to_value(input)?;
        Self::from_json(value)
    }
    /// Inspect the unchanged wire field.
    pub fn as_json(&self) -> &Value {
        &self.0
    }
}

/// An immutable, validated flat form schema. Submitted answers are not coerced
/// and schema defaults are never inserted into them.
#[derive(Debug)]
pub struct OpenAIForm {
    schema: Value,
    validator: jsonschema::Validator,
}

impl OpenAIForm {
    /// Validate a raw form and compile its answer validator once.
    /// `$schema` is preserved; validation uses JSON Schema Draft 2020-12.
    pub fn from_json(schema: Value) -> Result<Self, Error> {
        let root = schema
            .as_object()
            .ok_or_else(|| invalid("form must be an object"))?;
        allowed(root, &["$schema", "type", "properties", "required"], "form")?;
        if root.get("type") != Some(&json!("object")) {
            return Err(invalid("form.type must be object"));
        }
        let properties = root
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("form.properties must be an object"))?;
        validate_required(root, properties, "form")?;
        for (name, field) in properties {
            validate_field(field).map_err(|e| invalid(format!("form.properties.{name}: {e}")))?;
        }
        let validator = compile(&schema)?;
        Ok(Self { schema, validator })
    }

    /// Build a form from named fields and required property names.
    pub fn new(
        fields: impl IntoIterator<Item = (String, FormField)>,
        required: Vec<String>,
    ) -> Result<Self, Error> {
        let mut properties = serde_json::Map::new();
        for (name, field) in fields {
            if properties.insert(name.clone(), field.0).is_some() {
                return Err(invalid(format!("duplicate form field: {name}")));
            }
        }
        Self::from_json(json!({"type":"object","properties":properties,"required":required}))
    }

    /// Inspect the exact schema sent to the host, including all extension fields.
    pub fn as_json(&self) -> &Value {
        &self.schema
    }

    /// Validate a peer result, including primitive/string-array content and picker
    /// membership. Cancel and decline never supply accepted content.
    pub fn validate_result(&self, result: &FormResult) -> Result<(), Error> {
        if let FormResult::Accept { content } = result {
            for (name, value) in content {
                if !(value.is_string()
                    || value.is_boolean()
                    || value.is_number()
                    || value
                        .as_array()
                        .is_some_and(|a| a.iter().all(Value::is_string)))
                {
                    return Err(invalid(format!(
                        "form.content.{name} must be a primitive or string array"
                    )));
                }
            }
            validate(
                &self.validator,
                &Value::Object(content.clone()),
                "form.content",
            )?;
            for (name, field) in self.schema["properties"]
                .as_object()
                .expect("validated properties")
            {
                if let (Some(input), Some(value)) = (field.get("x-openai-input"), content.get(name))
                {
                    let input: ResourceInput = serde_json::from_value(input.clone())?;
                    for uri in selected(value)? {
                        if !input.permits(uri) {
                            return Err(invalid(format!(
                                "form.content.{name}: resource is not a supplied option"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Host response to an OpenAI form. Accepted values remain unmodified.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum FormResult {
    /// The user accepted the form.
    Accept {
        /// Primitive fields or string arrays, checked against the requested form.
        content: serde_json::Map<String, Value>,
    },
    /// The user cancelled the form.
    Cancel,
    /// The user declined the form.
    Decline,
}

/// Whether a peer advertises `extensions["openai/elicitation"].form` as an object.
pub fn supports_forms(capabilities: &ClientCapabilities) -> bool {
    capabilities
        .extensions
        .as_ref()
        .and_then(|e| e.get(ELICITATION_CAPABILITY_KEY))
        .and_then(|e| e.get("form"))
        .is_some_and(Value::is_object)
}

/// Elicit a form on a legacy direct MCP connection, validating the returned answer.
/// Returns a local error before sending if the host lacks the extension or uses
/// MCP 2026-07-28 or later. Use [`mrtr::FormService`] for modern form calls.
pub async fn elicit_input(
    context: &RequestContext<RoleServer>,
    message: impl Into<String>,
    form: &OpenAIForm,
) -> Result<FormResult, Error> {
    elicit_input_with_options(context, message, form, PeerRequestOptions::no_options()).await
}

/// Like [`elicit_input`], with rmcp timeout/cancellation request options.
pub async fn elicit_input_with_options(
    context: &RequestContext<RoleServer>,
    message: impl Into<String>,
    form: &OpenAIForm,
    options: PeerRequestOptions,
) -> Result<FormResult, Error> {
    if context
        .protocol_version()
        .is_some_and(|v| v.as_str() >= "2026-07-28")
    {
        return Err(Error::UnsupportedProtocol);
    }
    let capabilities = context
        .client_capabilities()
        .ok_or(Error::UnsupportedClient)?;
    if !supports_forms(&capabilities) {
        return Err(Error::UnsupportedClient);
    }
    let message = message.into();
    nonblank(&message, "form.message")?;
    let result = context
        .peer
        .send_request_with_option(
            ServerRequest::CustomRequest(CustomRequest::new(
                ELICITATION_METHOD,
                Some(json!({"mode":"form","message":message,"requestedSchema":form.schema})),
            )),
            options,
        )
        .await?
        .await_response()
        .await?;
    let result: FormResult = serde_json::from_value(serde_json::to_value(result)?)?;
    form.validate_result(&result)?;
    Ok(result)
}

fn validate_field(value: &Value) -> Result<(), Error> {
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
                #[derive(Deserialize)]
                struct Preview {
                    target: PreviewTarget,
                }
                let preview: Preview = serde_json::from_value(preview.clone())
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
    Ok(())
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
        for key in ["title", "description"] {
            if option.get(key).is_some_and(|v| !v.is_string()) {
                return Err(invalid(format!("option.{key} must be a string")));
            }
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
fn validate_uri(uri: &str, location: &str) -> Result<(), Error> {
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
