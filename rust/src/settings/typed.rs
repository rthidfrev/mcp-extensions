//! Typed settings values and non-null partial updates over the shared raw routes.
use super::{SettingsCapability, SettingsGroup, SettingsRegistration, SettingsTools};
use crate::{
    Error, ExtensionFuture,
    extensions::tool::internal,
    model::{self, Model, ModelField},
    validation::{failure, pointer},
};
use rmcp::{ErrorData, RoleServer, model::JsonObject, service::RequestContext};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{fmt, future::Future, marker::PhantomData, sync::Arc};

/// A complete settings value model and its matching partial update model.
///
/// Values must describe concrete primitive fields; `Patch` must describe the
/// same serialized names and exact field definitions, including titles and
/// descriptions, with every Rust type wrapped in `Option`. `None` means omitted,
/// never JSON `null`. Values are serialized and patches are deserialized; neither
/// needs the opposite trait. If a consumer also serializes patches, it should skip
/// absent fields.
/// Implement both [`Model`] descriptions by hand or use the optional derives.
/// The consumer decides how to authorize, merge and persist updates.
pub trait SettingsModel: Model + Serialize {
    /// A flat update model whose optional fields correspond to the complete values.
    type Patch: Model + DeserializeOwned;
}

/// Validated typed settings backed by the existing raw registration and routes.
///
/// Construction compiles the model and raw validators once, without installing
/// tools or performing application effects. Patch models must match complete
/// values exactly except for optionality; omission preserves the current value.
/// Unknown fields, nulls and empty patches are rejected before update callbacks.
pub struct TypedSettings<V: SettingsModel> {
    registration: SettingsRegistration,
    values_fields: Vec<ModelField>,
    patch_fields: Vec<ModelField>,
    model: PhantomData<fn() -> V>,
}

impl<V: SettingsModel> fmt::Debug for TypedSettings<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedSettings")
            .field("registration", &self.registration)
            .field("values_fields", &self.values_fields)
            .field("patch_fields", &self.patch_fields)
            .finish()
    }
}

impl<V: SettingsModel> TypedSettings<V> {
    /// Validate matching complete/partial models and their primitive schemas.
    ///
    /// Returns a local error for optional complete values, required patch fields,
    /// mismatched definitions, duplicate names or unsupported settings shapes.
    /// Arrays supported by forms remain unsupported for native settings.
    pub fn new() -> Result<Self, Error> {
        Self::validate_definition().map_err(Error::configuration)
    }

    fn validate_definition() -> Result<Self, Error> {
        let values_fields = model::fields::<V>()?;
        for field in &values_fields {
            if !field.is_required() {
                return Err(failure(
                    pointer(field.name()),
                    "settings values must use concrete required fields",
                ));
            }
        }
        let patch_fields = model::fields::<V::Patch>()?;
        for field in &patch_fields {
            if field.is_required() {
                return Err(failure(
                    pointer(field.name()),
                    "settings patch fields must be optional",
                ));
            }
            let complete = values_fields
                .iter()
                .find(|value| value.name() == field.name())
                .ok_or_else(|| {
                    failure(
                        pointer(field.name()),
                        "patch field is absent from settings values",
                    )
                })?;
            if complete.field().as_json() != field.field().as_json() {
                return Err(failure(
                    pointer(field.name()),
                    "patch field definition differs from settings values",
                ));
            }
        }
        for field in &values_fields {
            if !patch_fields
                .iter()
                .any(|patch| patch.name() == field.name())
            {
                return Err(failure(
                    pointer(field.name()),
                    "settings value is absent from the patch model",
                ));
            }
        }
        let registration = SettingsRegistration::new(model::schema(&values_fields))?;
        Ok(Self {
            registration,
            values_fields,
            patch_fields,
            model: PhantomData,
        })
    }

    /// Set distinct, nonblank read/update names using the raw validator.
    pub fn with_tool_names(
        self,
        read: impl Into<String>,
        update: impl Into<String>,
    ) -> Result<Self, Error> {
        Ok(Self {
            registration: self.registration.with_tool_names(read, update)?,
            ..self
        })
    }

    /// Validate and attach the native layout using the raw registration.
    pub fn with_layout(self, layout: Vec<SettingsGroup>) -> Result<Self, Error> {
        Ok(Self {
            registration: self.registration.with_layout(layout)?,
            ..self
        })
    }

    /// Inspect the validated raw schema registration without installing tools.
    pub fn registration(&self) -> &SettingsRegistration {
        &self.registration
    }

    /// Inspect names for collision preflight before installing the complete pair.
    pub fn capability(&self) -> &SettingsCapability {
        &self.registration.capability
    }

    /// Validate a nonempty partial replacement, including its final Serde decode.
    /// No callback or persistence occurs; false, zero and empty strings are retained.
    pub fn validate_patch(&self, set: &JsonObject) -> Result<(), Error> {
        self.decode_patch(set.clone()).map(|_| ())
    }

    /// Validate the raw patch first, then decode the optional Rust update model.
    /// Empty/unknown/null/invalid inputs are errors rather than omitted updates.
    /// Native numeric preparation follows [`crate::model::FieldType`]; rounded
    /// floats must still satisfy the field schema before callbacks can run.
    pub fn decode_patch(&self, set: JsonObject) -> Result<V::Patch, Error> {
        self.registration
            .validate_patch(&set)
            .map_err(|error| model::validation_error(error, &self.patch_fields, &set, true))?;
        model::decode(&self.patch_fields, set, true)
    }

    /// Serialize every effective value and check its Rust and raw schema domains.
    /// The returned object has all fields; serialization does not persist values.
    pub fn encode_values(&self, values: &V) -> Result<JsonObject, Error> {
        let values = encode(&self.values_fields, values)?;
        Ok(values)
    }

    /// Build typed read/update callbacks over the shared validated settings routes.
    ///
    /// `read` and `update` return every effective value; `update` receives the
    /// decoded partial model and owns atomic persistence. The raw route validates
    /// input before typed conversion, which completes before invoking `update`.
    /// Invalid serialized results produce internal MCP errors after callback
    /// effects, which this adapter cannot roll back. Contexts remain unchanged.
    pub fn into_tools<S, R, U, RFut, UFut>(self, read: R, update: U) -> SettingsTools<S>
    where
        S: Send + Sync + 'static,
        R: Fn(RequestContext<RoleServer>) -> RFut + Send + Sync + 'static,
        U: Fn(V::Patch, RequestContext<RoleServer>) -> UFut + Send + Sync + 'static,
        RFut: Future<Output = Result<V, ErrorData>> + Send + 'static,
        UFut: Future<Output = Result<V, ErrorData>> + Send + 'static,
    {
        self.into_tools_with_state(
            move |_, context| Box::pin(read(context)),
            move |_, patch, context| Box::pin(update(patch, context)),
        )
    }

    /// Build typed settings callbacks that borrow the original server across await.
    ///
    /// Return `Box::pin(async move { ... })` with the supplied `&S`. No consumer
    /// `Arc` is required; prepared field validators are shared internally. Input,
    /// output, context and effect behavior match [`Self::into_tools`]. Install the
    /// returned pair atomically before advertising its capability.
    pub fn into_tools_with_state<S, R, U>(self, read: R, update: U) -> SettingsTools<S>
    where
        S: Send + Sync + 'static,
        R: for<'a> Fn(&'a S, RequestContext<RoleServer>) -> ExtensionFuture<'a, V>
            + Send
            + Sync
            + 'static,
        U: for<'a> Fn(&'a S, V::Patch, RequestContext<RoleServer>) -> ExtensionFuture<'a, V>
            + Send
            + Sync
            + 'static,
    {
        let values_fields = Arc::new(self.values_fields);
        let patch_fields = Arc::new(self.patch_fields);
        let read_fields = values_fields.clone();
        let read = Arc::new(read);
        let update = Arc::new(update);
        self.registration.into_tools_with_state(
            move |server, context| {
                let fields = read_fields.clone();
                let read = read.clone();
                Box::pin(async move {
                    let values = read(server, context).await?;
                    encode(&fields, &values).map_err(internal)
                })
            },
            move |server, set, context| {
                let values_fields = values_fields.clone();
                let patch_fields = patch_fields.clone();
                let update = update.clone();
                Box::pin(async move {
                    let patch = model::decode::<V::Patch>(&patch_fields, set, true)
                        .map_err(ErrorData::from)?;
                    let values = update(server, patch, context).await?;
                    encode(&values_fields, &values).map_err(internal)
                })
            },
        )
    }
}

fn encode<V: Serialize>(fields: &[ModelField], values: &V) -> Result<JsonObject, Error> {
    let value = serde_json::to_value(values).map_err(|source| Error::ModelConversion {
        path: String::new(),
        source,
    })?;
    let Value::Object(values) = value else {
        return Err(failure("", "settings values must serialize as an object"));
    };
    model::check(fields, &values, false)?;
    Ok(values)
}
