//! Fixed settings envelopes and the schemas describing their payloads.
use rmcp::model::JsonObject;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{SettingsGroup, SettingsRegistration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateArguments {
    pub set: JsonObject,
}

#[derive(Serialize)]
pub(super) struct ReadResult<'a> {
    pub schema: &'a Value,
    pub values: &'a JsonObject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<&'a [SettingsGroup]>,
}

#[derive(Serialize)]
pub(super) struct UpdateResult<'a> {
    pub values: &'a JsonObject,
}

pub(super) fn values_schema(schema: &Value) -> Value {
    let mut complete = schema.clone();
    complete["required"] = json!(
        schema["properties"]
            .as_object()
            .expect("validated settings properties")
            .keys()
            .collect::<Vec<_>>()
    );
    complete["additionalProperties"] = json!(false);
    complete
}

pub(super) fn patch_schema(schema: &Value) -> Value {
    let mut patch = values_schema(schema);
    patch["required"] = json!([]);
    patch["minProperties"] = json!(1);
    patch
}

pub(super) fn read_input_schema() -> Value {
    json!({"type":"object","properties":{},"additionalProperties":false})
}

pub(super) fn read_output_schema(registration: &SettingsRegistration) -> Value {
    let layout_schema = match &registration.layout {
        Some(layout) => json!({"type":"array","items":{"type":"object"},"const":layout}),
        None => json!({"type":"array","items":{"type":"object"}}),
    };
    json!({
        "type": "object",
        "properties": {
            "schema": {"type": "object", "const": registration.schema},
            "layout": layout_schema,
            "values": values_schema(&registration.schema)
        },
        "required": ["schema", "values"]
    })
}

pub(super) fn update_input_schema(registration: &SettingsRegistration) -> Value {
    json!({
        "type":"object",
        "properties":{"set":patch_schema(&registration.schema)},
        "required":["set"],
        "additionalProperties":false
    })
}

pub(super) fn update_output_schema(registration: &SettingsRegistration) -> Value {
    json!({
        "type":"object",
        "properties":{"values":values_schema(&registration.schema)},
        "required":["values"]
    })
}
