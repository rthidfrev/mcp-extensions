//! Flat Rust model descriptions shared by typed forms, settings and optional derives.
//!
//! Implement [`Model`] by hand. Fields describe their actual
//! serialized names and Rust types. This deliberately supports the extension's
//! flat subset rather than projecting arbitrary JSON Schemas or Serde transforms.

mod annotations;
mod field;
mod value;

use annotations::annotate;
pub use field::ModelField;
pub use value::FieldType;

use crate::{
    Error,
    validation::{failure, pointer},
};
use rmcp::model::JsonObject;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Describe a flat model using its effective serialized field names.
///
/// Each field must agree with the model's Serde representation. Optional fields
/// use `Option<T>`; defaults never substitute for missing required wire values.
/// Unsupported types require an explicit, truthful [`FieldType`] implementation.
/// Custom Serde transformations must preserve the declared field domains; the SDK
/// checks wire values and native numeric rounding before deserializing the model.
pub trait Model {
    /// Construct supported fields and their annotations. Returns a local error
    /// for an invalid definition; no requests or application effects occur.
    fn fields() -> Result<Vec<ModelField>, Error>;
}

pub(crate) fn fields<M: Model>() -> Result<Vec<ModelField>, Error> {
    let fields = M::fields()?;
    let mut names = std::collections::HashSet::new();
    for field in &fields {
        if !names.insert(field.name()) {
            return Err(failure(pointer(field.name()), "duplicate model field"));
        }
    }
    Ok(fields)
}

pub(crate) fn schema(fields: &[ModelField]) -> Value {
    let properties: JsonObject = fields
        .iter()
        .map(|f| (f.name().into(), f.field().as_json().clone()))
        .collect();
    let required: Vec<_> = fields
        .iter()
        .filter(|f| f.is_required())
        .map(ModelField::name)
        .collect();
    json!({"type":"object","properties":properties,"required":required})
}

pub(crate) fn decode<T: DeserializeOwned>(
    fields: &[ModelField],
    mut values: JsonObject,
    partial: bool,
) -> Result<T, Error> {
    check(fields, &values, partial)?;
    for field in fields {
        if let Some(value) = values.get_mut(field.name()) {
            field.normalize(value)?;
        }
    }
    convert(values)
}

pub(crate) fn convert<T: DeserializeOwned>(values: JsonObject) -> Result<T, Error> {
    serde_json::from_value(Value::Object(values)).map_err(|source| Error::ModelConversion {
        path: String::new(),
        source,
    })
}

pub(crate) fn check(
    fields: &[ModelField],
    values: &JsonObject,
    partial: bool,
) -> Result<(), Error> {
    for name in values.keys() {
        if !fields.iter().any(|f| f.name() == name) {
            return Err(failure(pointer(name), "unknown model field"));
        }
    }
    for field in fields {
        match values.get(field.name()) {
            Some(value) => field.check(value)?,
            None if !partial && field.is_required() => {
                return Err(failure(pointer(field.name()), "missing required field"));
            }
            None => {}
        }
    }
    Ok(())
}

pub(crate) fn validation_error(
    error: Error,
    fields: &[ModelField],
    values: &JsonObject,
    partial: bool,
) -> Error {
    if let Err(detail) = check(fields, values, partial) {
        return detail;
    }
    match error {
        Error::Validation(_) => failure("", "values violate the extension contract"),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::FormField;
    use serde::Deserialize;

    #[test]
    fn accepted_model_deserializes_each_field_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        struct Counted;
        impl<'de> Deserialize<'de> for Counted {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer)?;
                CALLS.fetch_add(1, Ordering::SeqCst);
                Ok(Self)
            }
        }
        impl FieldType for Counted {
            fn schema() -> Result<Value, Error> {
                String::schema()
            }
        }
        #[derive(Deserialize)]
        struct Answer {
            _value: Counted,
        }
        impl Model for Answer {
            fn fields() -> Result<Vec<ModelField>, Error> {
                Ok(vec![ModelField::new::<Counted>("_value", "Value")?])
            }
        }
        let fields = fields::<Answer>().unwrap();
        decode::<Answer>(
            &fields,
            json!({"_value":"value"}).as_object().unwrap().clone(),
            false,
        )
        .unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);

        #[derive(serde::Serialize)]
        struct Values {
            _value: String,
        }
        #[derive(Deserialize)]
        struct Patch {
            _value: Option<Counted>,
        }
        impl Model for Values {
            fn fields() -> Result<Vec<ModelField>, Error> {
                Ok(vec![ModelField::new::<String>("_value", "Value")?])
            }
        }
        impl Model for Patch {
            fn fields() -> Result<Vec<ModelField>, Error> {
                Ok(vec![ModelField::new::<Option<Counted>>("_value", "Value")?])
            }
        }
        impl crate::settings::SettingsModel for Values {
            type Patch = Patch;
        }
        let settings = crate::settings::TypedSettings::<Values>::new().unwrap();
        settings
            .decode_patch(json!({"_value":"patch"}).as_object().unwrap().clone())
            .unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Answer {
        #[serde(rename = "a/b~c")]
        count: u8,
        note: Option<String>,
    }
    impl Model for Answer {
        fn fields() -> Result<Vec<ModelField>, Error> {
            Ok(vec![
                ModelField::new::<u8>("a/b~c", "Count")?,
                ModelField::new::<Option<String>>("note", "Note")?,
            ])
        }
    }

    #[test]
    fn handwritten_model_preserves_omission_and_reports_wire_paths() {
        let fields = fields::<Answer>().unwrap();
        assert_eq!(schema(&fields)["required"], json!(["a/b~c"]));
        let answer: Answer = decode(
            &fields,
            json!({"a/b~c":5}).as_object().unwrap().clone(),
            false,
        )
        .unwrap();
        assert_eq!(
            answer,
            Answer {
                count: 5,
                note: None
            }
        );
        for (value, path) in [
            (json!({"a/b~c":256}), "/a~1b~0c"),
            (json!({"a/b~c":1,"note":null}), "/note"),
            (json!({}), "/a~1b~0c"),
            (json!({"a/b~c":1,"extra":true}), "/extra"),
        ] {
            let error =
                decode::<Answer>(&fields, value.as_object().unwrap().clone(), false).unwrap_err();
            assert_eq!(error.path(), Some(path));
        }
    }

    #[test]
    fn annotations_cannot_weaken_rust_domain_or_introduce_unsupported_shapes() {
        let field = ModelField::new::<u8>("count", "Count").unwrap();
        assert!(
            field
                .with_annotations(json!({"maximum":500}).as_object().unwrap().clone())
                .is_err()
        );
        assert!(ModelField::new::<Option<Option<String>>>("note", "Note").is_err());
        assert!(FormField::try_from(json!({"type":"object"})).is_err());
        assert!(OpenAIFormForTest::try_from(json!({"type":"object","properties":{"name":{"type":"string"}},"required":["missing"]})).is_err());
        assert!(
            crate::settings::SettingsRegistration::try_from(
                json!({"type":"object","properties":{"x":{"type":"string","title":"X"}}})
            )
            .is_ok()
        );
    }

    #[test]
    fn typed_conversion_preserves_the_cause_and_field_pointer() {
        #[derive(Deserialize)]
        struct Encoded {
            #[serde(rename = "wire_name")]
            _value: bool,
        }
        impl Model for Encoded {
            fn fields() -> Result<Vec<ModelField>, Error> {
                Ok(vec![ModelField::new::<bool>("wire_name", "Value")?])
            }
        }
        // A handwritten model is responsible for its Serde representation.
        // A mismatch must fail conversion rather than silently coerce its data.
        #[derive(Deserialize)]
        struct Wrong {
            #[serde(rename = "wire_name")]
            _value: u8,
        }
        let error = decode::<Wrong>(
            &fields::<Encoded>().unwrap(),
            json!({"wire_name":true}).as_object().unwrap().clone(),
            false,
        )
        .err()
        .unwrap();
        assert!(matches!(error, Error::ModelConversion { .. }));
        assert_eq!(error.path(), Some(""));
        assert!(std::error::Error::source(&error).is_some());
    }
    use crate::forms::OpenAIForm as OpenAIFormForTest;
}
