use crate::Error;
use rmcp::{
    ErrorData,
    model::{CallToolResult, Tool, ToolAnnotations},
};
use serde_json::Value;
use std::sync::Arc;

pub(crate) fn make_tool(name: &str, input: Value, output: Option<Value>, readonly: bool) -> Tool {
    let mut tool = Tool::new(
        name.to_owned(),
        name.to_owned(),
        Arc::new(input.as_object().expect("object schema").clone()),
    );
    tool.output_schema = output.map(|v| Arc::new(v.as_object().expect("object schema").clone()));
    let mut annotations = ToolAnnotations::default();
    annotations.read_only_hint = Some(readonly);
    tool.annotations = Some(annotations);
    tool
}
pub(crate) fn structured(value: Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![]);
    result.structured_content = Some(value);
    result
}
pub(crate) fn internal(error: Error) -> ErrorData {
    error.into_internal()
}
