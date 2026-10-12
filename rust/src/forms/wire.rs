//! Fixed OpenAI form envelopes; schemas and extension metadata remain open JSON.
use std::collections::BTreeMap;

use rmcp::model::{MetaObject, ResultType};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::OpenAIForm;

#[derive(Serialize, Deserialize)]
pub(super) struct PreviewMetadata<T> {
    pub target: T,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Form,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestParams {
    mode: Mode,
    message: String,
    requested_schema: Value,
}

impl RequestParams {
    pub(super) fn new(message: String, form: &OpenAIForm) -> Self {
        Self {
            mode: Mode::Form,
            message,
            requested_schema: form.as_json().clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct InputRequest {
    method: &'static str,
    params: ModernRequestParams,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModernRequestParams {
    mode: Mode,
    message: String,
    requested_schema: Value,
    #[serde(rename = "_meta")]
    meta: Value,
}

impl InputRequest {
    pub(super) fn new(message: String, form: &OpenAIForm) -> Self {
        Self {
            method: "elicitation/create",
            params: ModernRequestParams {
                mode: Mode::Form,
                message,
                requested_schema: serde_json::json!({"type":"object","properties":{}}),
                meta: serde_json::json!({"openai/elicitation":{"requestedSchema":form.as_json()}}),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InputRequired<'a> {
    pub result_type: ResultType,
    pub input_requests: &'a BTreeMap<String, InputRequest>,
    pub request_state: &'a str,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<&'a MetaObject>,
}
