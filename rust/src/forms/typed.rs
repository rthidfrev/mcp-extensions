use std::{fmt, marker::PhantomData};

use rmcp::{
    RoleServer,
    model::{CallToolRequestParams, JsonObject},
    service::{PeerRequestOptions, RequestContext},
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{FormResult, OpenAIForm, mrtr};
use crate::{
    Error,
    model::{self, Model, ModelField},
    validation::{failure, pointer},
};

/// A supported flat form whose accepted answers decode into a Rust model.
///
/// The model describes effective Serde names and Rust domains. `Option<T>` means
/// omission is allowed, while explicit `null` remains unsupported. Required
/// wire values are never supplied from schema or Serde defaults. Unlike raw
/// [`OpenAIForm`], typed decoding also rejects unknown answer properties.
pub struct TypedForm<T: Model + DeserializeOwned> {
    fields: Vec<ModelField>,
    raw: OpenAIForm,
    marker: PhantomData<fn() -> T>,
}

impl<T: Model + DeserializeOwned> fmt::Debug for TypedForm<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TypedForm")
            .field("fields", &self.fields)
            .field("raw", &self.raw)
            .finish()
    }
}

/// A validated form result with Rust content only when the user accepts.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedFormResult<T> {
    /// The wire answer was validated, then decoded using the model's Serde implementation.
    Accept {
        /// The application's Rust model.
        content: T,
    },
    /// The user cancelled; no model conversion occurs.
    Cancel,
    /// The user declined; no model conversion occurs.
    Decline,
}

/// An initial tool call or a typed response from a modern form round trip.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedFormInput<T> {
    /// Neither continuation field was present on the original request.
    Initial,
    /// A keyed response passed raw form validation and typed decoding.
    Response {
        /// Untrusted opaque state, preserved exactly. The application must verify
        /// caller/argument binding, integrity, expiry and single consumption.
        request_state: String,
        /// Accept, cancel or decline; only accept contains a Rust model.
        result: TypedFormResult<T>,
    },
}

impl<T: Model + DeserializeOwned> TypedForm<T> {
    /// Construct and validate the model's supported flat form.
    /// Invalid model fields, duplicate names and unsupported schemas return
    /// local errors before any request or application effect.
    pub fn new() -> Result<Self, Error> {
        let fields = model::fields::<T>().map_err(Error::configuration)?;
        let raw = form_from_fields(&fields)?;
        Ok(Self {
            fields,
            raw,
            marker: PhantomData,
        })
    }

    /// Borrow the validated raw form for existing protocol helpers, including
    /// [`mrtr::FormInputRequired`]. This does not request input or register a tool.
    pub fn raw_form(&self) -> &OpenAIForm {
        &self.raw
    }

    /// Inspect the exact outgoing schema, including all runtime annotations.
    pub fn as_json(&self) -> &Value {
        self.raw.as_json()
    }

    /// Consume this form and annotate one effective serialized field name.
    ///
    /// The resulting form retains the model's requiredness and type. Numeric
    /// ranges and enum choices may narrow, but cannot widen the Rust domain.
    /// Supported annotations include suggestions and runtime resource options.
    /// Unknown names, unsupported annotations or incompatible model constraints
    /// return a local error; no request or application effect occurs.
    pub fn with_field_annotations(
        mut self,
        name: &str,
        annotations: JsonObject,
    ) -> Result<Self, Error> {
        let index = self
            .fields
            .iter()
            .position(|field| field.name() == name)
            .ok_or_else(|| failure(pointer(name), "unknown form field").configuration())?;
        let field = self.fields.remove(index).with_annotations(annotations)?;
        self.fields.insert(index, field);
        self.raw = form_from_fields(&self.fields)?;
        Ok(self)
    }

    /// Validate accepted fields through the shared form schemas, then decode `T` once.
    ///
    /// Raw constraints and resource membership are checked before model
    /// conversion. Unknown properties, Rust-domain violations and inconsistent
    /// Serde models return errors. Native integral numbers are normalized and
    /// rounded floats must still satisfy the schema. Strings and nulls are
    /// not coerced; defaults are never inserted. Cancel/decline bypass conversion.
    pub fn decode_result(&self, result: FormResult) -> Result<TypedFormResult<T>, Error> {
        match result {
            FormResult::Accept { content } => Ok(TypedFormResult::Accept {
                content: model::decode(&self.fields, content, false)?,
            }),
            FormResult::Cancel => Ok(TypedFormResult::Cancel),
            FormResult::Decline => Ok(TypedFormResult::Decline),
        }
    }

    /// Request this form through the existing legacy direct elicitation flow.
    /// Uses the shared request transport with default rmcp request options
    /// and the same validation/decoding as [`Self::decode_result`].
    /// Unsupported capability/protocol, request failures, invalid answers and
    /// model conversion failures return errors. Authorization remains with the
    /// application. Modern tool round trips use [`Self::read_input`] instead.
    pub async fn elicit_input(
        &self,
        context: &RequestContext<RoleServer>,
        message: impl Into<String>,
    ) -> Result<TypedFormResult<T>, Error> {
        self.elicit_input_with_options(context, message, PeerRequestOptions::no_options())
            .await
    }

    /// Like [`Self::elicit_input`], forwarding rmcp request options such as timeouts
    /// unchanged to [`super::elicit_input_with_options`].
    pub async fn elicit_input_with_options(
        &self,
        context: &RequestContext<RoleServer>,
        message: impl Into<String>,
        options: PeerRequestOptions,
    ) -> Result<TypedFormResult<T>, Error> {
        let result = super::request_input(context, message, &self.raw, options).await?;
        self.decode_result(result).map_err(Error::invalid_response)
    }

    /// Read a keyed response through the shared modern continuation parser,
    /// then validate and decode with [`Self::decode_result`].
    ///
    /// Raw continuation checks and request state are preserved. Missing or
    /// malformed responses and invalid typed content return errors. This does
    /// not look up state, authorize the request or perform application effects;
    /// retain the form definition bound to the saved continuation.
    pub fn read_input(
        &self,
        params: &CallToolRequestParams,
        key: &str,
    ) -> Result<TypedFormInput<T>, Error> {
        let input = mrtr::read_response(params, key)?;
        match input {
            mrtr::FormInput::Initial => Ok(TypedFormInput::Initial),
            mrtr::FormInput::Response {
                request_state,
                result,
            } => Ok(TypedFormInput::Response {
                request_state,
                result: self.decode_result(result)?,
            }),
        }
    }
}

fn form_from_fields(fields: &[ModelField]) -> Result<OpenAIForm, Error> {
    OpenAIForm::new(
        fields
            .iter()
            .map(|field| (field.name().to_owned(), field.field().clone())),
        fields
            .iter()
            .filter(|field| field.is_required())
            .map(|field| field.name().to_owned())
            .collect(),
    )
}
