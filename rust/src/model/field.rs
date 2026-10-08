use super::FieldType;
use crate::{
    Error,
    forms::FormField,
    validation::{failure, pointer},
};
use rmcp::model::JsonObject;
use serde_json::Value;

/// A named, validated field whose requiredness and wire domain follow its Rust type.
#[derive(Debug)]
pub struct ModelField {
    name: String,
    field: FormField,
    required: bool,
    normalize: fn(&mut Value) -> Result<(), Error>,
}

impl ModelField {
    /// Describe a concrete field or an optional `Option<T>` field.
    /// The name must match Serde's effective name, and the title must be nonblank.
    /// No nested objects, nullable values or implicit defaults are introduced.
    pub fn new<T: FieldType>(
        name: impl Into<String>,
        title: impl Into<String>,
    ) -> Result<Self, Error> {
        Self::describe::<T>(name.into(), title.into()).map_err(Error::configuration)
    }

    fn describe<T: FieldType>(name: String, title: String) -> Result<Self, Error> {
        if title.trim().is_empty() {
            return Err(failure(pointer(&name), "field title must not be blank"));
        }
        let mut schema = T::schema()?;
        let object = schema
            .as_object_mut()
            .ok_or_else(|| failure(pointer(&name), "field schema must be an object"))?;
        object.insert("title".into(), Value::String(title));
        let field =
            FormField::try_from(schema).map_err(|e| failure(pointer(&name), e.to_string()))?;
        Ok(Self {
            name,
            field,
            required: !T::OPTIONAL,
            normalize: T::normalize_value,
        })
    }

    /// Add supported form annotations without widening the existing field domain.
    /// Titled choices can replace string enums or string-array item constraints
    /// if every choice satisfies the original schema. Bounds, lengths, cardinality
    /// and multiples can be tightened. Existing patterns, formats and resource
    /// definitions cannot be replaced. The result passes raw field validation,
    /// including defaults and previews; invalid annotations are configuration errors.
    pub fn with_annotations(mut self, annotations: JsonObject) -> Result<Self, Error> {
        self.field = super::annotate(&self.field, annotations)
            .map_err(|e| failure(pointer(&self.name), e.to_string()).configuration())?;
        Ok(self)
    }

    /// Effective serialized field name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Exact supported wire definition, including runtime annotations.
    pub fn field(&self) -> &FormField {
        &self.field
    }
    /// Whether omitting this field is invalid for a complete form/model.
    pub fn is_required(&self) -> bool {
        self.required
    }

    pub(crate) fn check(&self, value: &Value) -> Result<(), Error> {
        let path = pointer(&self.name);
        if let Err(error) = self.field.validator().validate(value) {
            // The library's formatted error may contain submitted data. Report
            // the contract failure and pointer without including that value.
            return Err(failure(
                format!("{path}{}", error.instance_path()),
                "value violates the field schema",
            ));
        }
        if let Some(input) = self.field.as_json().get("x-openai-input") {
            crate::forms::validate_resource_selection(input, value, "field")
                .map_err(|error| failure(path.clone(), error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn normalize(&self, value: &mut Value) -> Result<(), Error> {
        (self.normalize)(value).map_err(|error| match error {
            Error::ModelValidation { path, message } => {
                failure(format!("{}{path}", pointer(&self.name)), message)
            }
            Error::ModelConversion { path, source } => Error::ModelConversion {
                path: format!("{}{path}", pointer(&self.name)),
                source,
            },
            other => other,
        })?;
        self.check(value)
    }
}
