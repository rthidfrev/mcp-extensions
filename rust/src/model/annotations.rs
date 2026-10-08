//! Monotone composition of typed fields with supported form annotations.
//!
//! Form schema validation owns supported shapes. This adapter only establishes
//! that replacing a constraint or choice representation preserves the prior domain.
use crate::{
    Error,
    forms::FormField,
    validation::{compile, failure, pointer},
};
use rmcp::model::JsonObject;
use serde_json::{Value, json};

pub(super) fn annotate(base: &FormField, annotations: JsonObject) -> Result<FormField, Error> {
    let mut value = base.as_json().as_object().expect("validated field").clone();
    merge(&mut value, annotations, "")?;
    // Keep all protocol shape, option, resource and default checks in their owner.
    FormField::try_from(Value::Object(value))
}

fn merge(value: &mut JsonObject, annotations: JsonObject, path: &str) -> Result<(), Error> {
    if annotations.contains_key("enum") && annotations.contains_key("oneOf") {
        return Err(failure(
            path,
            "annotations must select one choice representation",
        ));
    }
    if let Some(options) = annotations.get("oneOf") {
        choices_within(value, options, true, &at(path, "oneOf"))?;
        value.remove("enum");
        value.remove("enumNames");
    } else if let Some(options) = annotations.get("enum") {
        choices_within(value, options, false, &at(path, "enum"))?;
        // Reordering/narrowing legacy choices must keep their labels aligned.
        if !annotations.contains_key("enumNames")
            && let (Some(old), Some(names), Some(new)) = (
                value.get("enum").and_then(Value::as_array),
                value.get("enumNames").and_then(Value::as_array),
                options.as_array(),
            )
        {
            let names = new
                .iter()
                .map(|option| {
                    names[old
                        .iter()
                        .position(|v| v == option)
                        .expect("checked choice")]
                    .clone()
                })
                .collect();
            value.insert("enumNames".into(), Value::Array(names));
        }
        value.remove("oneOf");
    }
    for (key, annotation) in annotations {
        let location = at(path, &key);
        if key == "items" {
            let target = value
                .get_mut("items")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| failure(&location, "items must remain an object"))?;
            let additions = annotation
                .as_object()
                .ok_or_else(|| failure(&location, "items annotations must be an object"))?;
            merge_items(target, additions.clone(), &location)?;
            continue;
        }
        if let Some(previous) = value.get(&key)
            && previous != &annotation
            && !matches!(
                key.as_str(),
                "title"
                    | "description"
                    | "default"
                    | "enumNames"
                    | "x-openai-suggestions"
                    | "enum"
                    | "oneOf"
            )
        {
            let bound = match key.as_str() {
                "minimum" | "exclusiveMinimum" | "minLength" | "minItems" => "minimum",
                "maximum" | "exclusiveMaximum" | "maxLength" | "maxItems" => "maximum",
                "multipleOf" => "multipleOf",
                "uniqueItems" if previous == &json!(false) && annotation == json!(true) => {
                    value.insert(key, annotation);
                    continue;
                }
                _ => {
                    return Err(failure(
                        location,
                        "annotation cannot replace a model constraint",
                    ));
                }
            };
            // Compare numbers with the shared validator: converting integer bounds
            // to f64 would lose distinctions near i64/u64 limits.
            let mut bounds = json!({"type":"number"});
            bounds[bound] = previous.clone();
            if !compile(&bounds)?.is_valid(&annotation) {
                return Err(failure(location, "annotation cannot widen a model domain"));
            }
        }
        value.insert(key, annotation);
    }
    Ok(())
}

fn merge_items(value: &mut JsonObject, annotations: JsonObject, path: &str) -> Result<(), Error> {
    if annotations.is_empty() {
        return Ok(());
    }
    if let Some(options) = annotations.get("anyOf") {
        if annotations.len() != 1 {
            return Err(failure(
                path,
                "titled array choices cannot have sibling item keywords",
            ));
        }
        choices_within(value, options, true, &at(path, "anyOf"))?;
        // The supported titled item representation permits only anyOf. Every
        // new const satisfies the old item schema, so its constraints are redundant.
        *value = json!({"anyOf":options})
            .as_object()
            .expect("object")
            .clone();
        return Ok(());
    }
    if value.contains_key("anyOf") {
        let options = annotations.get("enum").ok_or_else(|| {
            failure(
                path,
                "titled array choices require an enum or anyOf replacement",
            )
        })?;
        choices_within(value, options, false, &at(path, "enum"))?;
        *value = json!({"type":"string"})
            .as_object()
            .expect("object")
            .clone();
    }
    merge(value, annotations, path)
}

fn choices_within(
    base: &JsonObject,
    options: &Value,
    titled: bool,
    path: &str,
) -> Result<(), Error> {
    let options = options
        .as_array()
        .ok_or_else(|| failure(path, "choices must be an array"))?;
    let validator = compile(&Value::Object(base.clone()))?;
    for (index, option) in options.iter().enumerate() {
        let option = if titled {
            option.get("const")
        } else {
            Some(option)
        }
        .filter(|v| v.is_string())
        .ok_or_else(|| failure(format!("{path}/{index}"), "choice must name a string value"))?;
        if !validator.is_valid(option) {
            return Err(failure(
                format!("{path}/{index}"),
                "choice lies outside the existing model domain",
            ));
        }
    }
    Ok(())
}

fn at(path: &str, key: &str) -> String {
    format!("{path}{}", pointer(key))
}
