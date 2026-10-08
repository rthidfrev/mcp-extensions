use rmcp::model::Resource;
use serde_json::{Value, json};

use super::{FormField, ResourceInput, ResourceKind, UserResourceOptions};
use crate::Error;

impl FormField {
    /// Build a field whose answer is an array of unrestricted strings.
    pub fn string_array(title: impl Into<String>) -> Self {
        Self::from_json(json!({"type":"array","title":title.into(),"items":{"type":"string"}}))
            .expect("SDK string array schema")
    }

    /// Build a string field restricted to the supplied values, in display order.
    /// Returns an error if the enum violates the existing form schema validation.
    pub fn string_enum(
        title: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, Error> {
        let values: Vec<String> = values.into_iter().map(Into::into).collect();
        let mut field = Self::string(title).schema;
        field["enum"] = json!(values);
        Self::from_json(field)
    }

    /// Build a string selector from `(value, title)` choices, in display order.
    /// Answers must match exactly one titled `const` option. Returns an error if
    /// the resulting `oneOf` schema fails the existing form validation.
    pub fn string_choices(
        title: impl Into<String>,
        choices: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Result<Self, Error> {
        let mut field = Self::string(title).schema;
        field["oneOf"] = titled_options(choices);
        Self::from_json(field)
    }

    /// Build a string field with `(value, title)` suggestions, in display order.
    /// Suggestions do not restrict answers to those values. Returns an error if
    /// the annotations fail the existing form validation.
    pub fn string_suggestions(
        title: impl Into<String>,
        suggestions: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Result<Self, Error> {
        let mut field = Self::string(title).schema;
        field["x-openai-suggestions"] = titled_options(suggestions);
        Self::from_json(field)
    }

    /// Build a string array with `(value, title)` suggestions on its items.
    /// Users can supply additional strings. Returns an error if the annotations
    /// fail the existing form validation.
    pub fn string_array_suggestions(
        title: impl Into<String>,
        suggestions: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Result<Self, Error> {
        let mut field = Self::string_array(title).schema;
        field["items"]["x-openai-suggestions"] = titled_options(suggestions);
        Self::from_json(field)
    }

    /// Build a resource picker whose answer is one URI string.
    /// Preserves resource options and their metadata. Returns the same validation
    /// errors as [`Self::resource`], including rejection of a selection mode.
    pub fn single_resource(title: impl Into<String>, input: ResourceInput) -> Result<Self, Error> {
        Self::resource(title, input, false)
    }

    /// Build a resource picker whose answer is an array of URI strings.
    /// Preserves resource options, metadata, user options and selection mode.
    /// Returns the same validation errors as [`Self::resource`].
    pub fn multiple_resources(
        title: impl Into<String>,
        input: ResourceInput,
    ) -> Result<Self, Error> {
        Self::resource(title, input, true)
    }
}

impl ResourceInput {
    /// Configure a canonical resource input allowing user-selected files.
    /// Supplied resources and host-interpreted accept filters are preserved.
    /// This does not open a picker; the enclosing form field validates resources,
    /// and the host determines whether uploads are supported.
    pub fn files(options: Vec<Resource>, accept: Option<Vec<String>>) -> Self {
        Self {
            user_options: Some(UserResourceOptions {
                kind: Some(ResourceKind::File),
                accept,
            }),
            ..Self::new(options)
        }
    }

    /// Configure a canonical resource input allowing user-selected directories.
    /// Supplied resources are preserved. This does not open a picker; the enclosing
    /// form field validates resources, and the host determines picker support.
    pub fn directories(options: Vec<Resource>) -> Self {
        Self {
            user_options: Some(UserResourceOptions {
                kind: Some(ResourceKind::Directory),
                accept: None,
            }),
            ..Self::new(options)
        }
    }
}

fn titled_options(
    options: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
) -> Value {
    Value::Array(
        options
            .into_iter()
            .map(|(value, title)| json!({"const":value.into(),"title":title.into()}))
            .collect(),
    )
}
