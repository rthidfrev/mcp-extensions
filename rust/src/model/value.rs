use crate::{Error, validation::failure};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// A Rust field's supported, non-null flat wire schema.
///
/// Implementations must describe their actual Serde representation. Built-ins
/// cover primitives, string arrays and one `Option` wrapper. String enums can
/// implement this by hand or use the optional `StringEnum` derive.
pub trait FieldType {
    /// `true` only when omission represents an optional field.
    const OPTIONAL: bool = false;
    /// Return a supported field schema. Titles and additional annotations are
    /// attached by [`super::ModelField`]; nullable schemas remain unsupported.
    fn schema() -> Result<Value, Error>;

    /// Prepare a schema-valid wire value for this type's Serde representation.
    ///
    /// The default leaves custom field values unchanged. Native integers accept
    /// mathematically integral JSON numbers within their Rust bounds; floats
    /// use Serde's rounding. The SDK validates the prepared value again before
    /// decoding the model. Overrides must perform no application effects and
    /// return validation/conversion errors with paths relative to this field.
    fn normalize_value(_value: &mut Value) -> Result<(), Error> {
        Ok(())
    }
}

impl FieldType for String {
    fn schema() -> Result<Value, Error> {
        Ok(json!({"type":"string"}))
    }
}
impl FieldType for bool {
    fn schema() -> Result<Value, Error> {
        Ok(json!({"type":"boolean"}))
    }
}
impl FieldType for Vec<String> {
    fn schema() -> Result<Value, Error> {
        Ok(json!({"type":"array","items":{"type":"string"}}))
    }
}
impl FieldType for f64 {
    fn schema() -> Result<Value, Error> {
        Ok(json!({"type":"number"}))
    }

    fn normalize_value(value: &mut Value) -> Result<(), Error> {
        normalize_float::<Self>(value)
    }
}
impl FieldType for f32 {
    fn schema() -> Result<Value, Error> {
        Ok(json!({"type":"number","minimum":-f32::MAX,"maximum":f32::MAX}))
    }

    fn normalize_value(value: &mut Value) -> Result<(), Error> {
        normalize_float::<Self>(value)
    }
}

fn normalize_float<T: DeserializeOwned + Into<f64>>(value: &mut Value) -> Result<(), Error> {
    // Follow Serde's direct integer-to-f32 conversion: going through f64 first
    // can round twice and change the result for large integers.
    let rounded: T =
        serde_json::from_value(value.clone()).map_err(|source| Error::ModelConversion {
            path: String::new(),
            source,
        })?;
    let rounded = rounded.into();
    if !rounded.is_finite() {
        return Err(failure(
            "",
            "value is outside the finite floating-point domain",
        ));
    }
    // Widen f32 exactly, even when a consumer enables arbitrary_precision.
    *value = Value::from(rounded);
    Ok(())
}

macro_rules! integers {
    ($signed:literal; $($ty:ty),+ $(,)?) => {$(
        impl FieldType for $ty {
            fn schema() -> Result<Value, Error> {
                Ok(json!({"type":"integer","minimum":<$ty>::MIN,"maximum":<$ty>::MAX}))
            }

            fn normalize_value(value: &mut Value) -> Result<(), Error> {
                normalize_integer(value, $signed)
            }
        }
    )+};
}
integers!(true; i8, i16, i32, i64, isize);
integers!(false; u8, u16, u32, u64, usize);

fn normalize_integer(value: &mut Value, signed: bool) -> Result<(), Error> {
    let Value::Number(number) = value else {
        return Err(failure("", "integer field requires a JSON number"));
    };
    if !number.is_f64() {
        // Preserve existing i64/u64 values, including exact values above 2^53.
        return Ok(());
    }
    let number = number
        .as_f64()
        .filter(|number| number.is_finite() && number.fract() == 0.0)
        .ok_or_else(|| failure("", "integer field requires a finite integral value"))?;
    // MAX as f64 rounds to the next power of two. The upper bounds must be
    // exclusive so the following casts never saturate or lose the represented value.
    let signed_limit = 9_223_372_036_854_775_808.0;
    let unsigned_limit = 18_446_744_073_709_551_616.0;
    *value = if signed && (-signed_limit..signed_limit).contains(&number) {
        Value::from(number as i64)
    } else if !signed && (0.0..unsigned_limit).contains(&number) {
        Value::from(number as u64)
    } else {
        return Err(failure("", "integer value is outside its Rust domain"));
    };
    Ok(())
}

impl<T: FieldType> FieldType for Option<T> {
    const OPTIONAL: bool = true;
    fn schema() -> Result<Value, Error> {
        if T::OPTIONAL {
            return Err(failure("", "nested optional fields are unsupported"));
        }
        T::schema()
    }

    fn normalize_value(value: &mut Value) -> Result<(), Error> {
        T::normalize_value(value)
    }
}
