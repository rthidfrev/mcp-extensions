//! Lossless extended form schemas, direct elicitation and modern tool round trips.
//!
//! rmcp's standard `ElicitationSchema` cannot represent all OpenAI fields. This
//! module keeps the wire schema as JSON instead of converting through that type.
use rmcp::{
    RoleServer,
    model::{ClientCapabilities, CustomRequest, Resource, ServerRequest},
    service::{PeerRequestOptions, RequestContext},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
mod convenience;
mod schema;
mod wire;
pub(crate) use schema::validate_resource_selection;
use schema::{validate_field, validate_uri};
pub mod mrtr;

use crate::{
    Error,
    error::{invalid, nonblank},
    validation::{allowed, compile, validate, validate_required},
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
        self.validate().map_err(Error::configuration)?;
        meta.0.insert(
            PREVIEW_META_KEY.into(),
            serde_json::to_value(wire::PreviewMetadata { target: self })
                .map_err(|source| Error::from(source).configuration())?,
        );
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
pub struct FormField {
    schema: Value,
    #[serde(skip)]
    validator: std::sync::Arc<jsonschema::Validator>,
}

impl TryFrom<Value> for FormField {
    type Error = Error;
    fn try_from(value: Value) -> Result<Self, Error> {
        Self::from_json(value)
    }
}

impl FormField {
    /// Validate a raw field without stripping OpenAI extensions or constraints.
    /// Supports primitives, enums, string arrays, suggestions and resource selectors.
    /// Unknown keywords, nested objects, references and unsupported inputs are rejected.
    pub fn from_json(value: Value) -> Result<Self, Error> {
        let validator = std::sync::Arc::new(validate_field(&value).map_err(Error::configuration)?);
        Ok(Self {
            schema: value,
            validator,
        })
    }
    /// Build a plain string field with optional length/pattern annotations added via JSON.
    pub fn string(title: impl Into<String>) -> Self {
        Self::from_json(json!({"type":"string","title":title.into()})).expect("SDK string schema")
    }
    /// Build a boolean field.
    pub fn boolean(title: impl Into<String>) -> Self {
        Self::from_json(json!({"type":"boolean","title":title.into()})).expect("SDK boolean schema")
    }
    /// Build a numeric field.
    pub fn number(title: impl Into<String>) -> Self {
        Self::from_json(json!({"type":"number","title":title.into()})).expect("SDK number schema")
    }
    /// Build an integer field.
    pub fn integer(title: impl Into<String>) -> Self {
        Self::from_json(json!({"type":"integer","title":title.into()})).expect("SDK integer schema")
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
        &self.schema
    }

    pub(crate) fn validator(&self) -> &jsonschema::Validator {
        &self.validator
    }
}

/// An immutable, validated flat form schema. Submitted answers are not coerced
/// and schema defaults are never inserted into them.
#[derive(Debug)]
pub struct OpenAIForm {
    schema: Value,
    validator: jsonschema::Validator,
}

impl TryFrom<Value> for OpenAIForm {
    type Error = Error;
    fn try_from(value: Value) -> Result<Self, Error> {
        Self::from_json(value)
    }
}

impl OpenAIForm {
    /// Validate a raw form and compile its answer validator once.
    /// `$schema` is preserved; validation uses JSON Schema Draft 2020-12.
    pub fn from_json(schema: Value) -> Result<Self, Error> {
        Self::validate_definition(schema).map_err(Error::configuration)
    }

    fn validate_definition(schema: Value) -> Result<Self, Error> {
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
            if properties.insert(name.clone(), field.schema).is_some() {
                return Err(invalid(format!("duplicate form field: {name}")).configuration());
            }
        }
        let schema = json!({"type":"object","properties":properties,"required":required});
        validate_required(
            schema.as_object().expect("object schema"),
            schema["properties"].as_object().expect("field map"),
            "form",
        )
        .map_err(Error::configuration)?;
        let validator = compile(&schema).map_err(Error::configuration)?;
        Ok(Self { schema, validator })
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
                    validate_resource_selection(input, value, &format!("form.content.{name}"))?;
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

pub(crate) fn required_capabilities() -> ClientCapabilities {
    let mut capabilities = ClientCapabilities::default();
    capabilities.extensions = Some(
        [(
            ELICITATION_CAPABILITY_KEY.into(),
            json!({"form": {}}).as_object().expect("object").clone(),
        )]
        .into(),
    );
    capabilities
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

/// Like [`elicit_input`], forwarding rmcp request options such as timeouts.
///
/// Parent request cancellation is not automatically propagated to this wait.
/// The server owns cancellation handling and must check it before effects.
pub async fn elicit_input_with_options(
    context: &RequestContext<RoleServer>,
    message: impl Into<String>,
    form: &OpenAIForm,
    options: PeerRequestOptions,
) -> Result<FormResult, Error> {
    let result = request_input(context, message, form, options).await?;
    form.validate_result(&result)
        .map_err(Error::invalid_response)?;
    Ok(result)
}

async fn request_input(
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
    nonblank(&message, "form.message").map_err(Error::configuration)?;
    let result = context
        .peer
        .send_request_with_option(
            ServerRequest::CustomRequest(CustomRequest::new(
                ELICITATION_METHOD,
                Some(serde_json::to_value(wire::RequestParams::new(
                    message, form,
                ))?),
            )),
            options,
        )
        .await?
        .await_response()
        .await?;
    let result: FormResult = serde_json::from_value(serde_json::to_value(result)?)
        .map_err(|source| Error::from(source).invalid_response())?;
    Ok(result)
}
