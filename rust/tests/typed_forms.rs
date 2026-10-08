use openai_mcp_extensions::{
    Error,
    forms::{
        FormField, FormResult, TypedForm, TypedFormInput, TypedFormResult,
        mrtr::{FormInput, FormInputRequired},
    },
    model::{FieldType, Model, ModelField},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{CallToolRequestParams, CallToolResponse, CallToolResult, JsonObject, ProtocolVersion},
    service::RequestContext,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[allow(dead_code)] // Reuse the shared RPC fixture; this file only tests legacy transport.
#[path = "support/wire.rs"]
mod wire;

fn object(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

fn accept(content: Value) -> FormResult {
    FormResult::Accept {
        content: object(content),
    }
}

#[derive(Debug, Deserialize, PartialEq)]
enum Unit {
    #[serde(rename = "mm")]
    Metric,
    #[serde(rename = "in")]
    Imperial,
}

impl FieldType for Unit {
    fn schema() -> Result<Value, Error> {
        Ok(json!({"type":"string","enum":["mm","in"]}))
    }
}

fn default_count() -> u8 {
    7
}

#[derive(Debug, Deserialize, PartialEq)]
struct Answer {
    #[serde(rename = "a/b~count", default = "default_count")]
    count: u8,
    unit: Unit,
    note: Option<String>,
    labels: Vec<String>,
    ratio: f64,
}

impl Model for Answer {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<u8>("a/b~count", "Count")?,
            ModelField::new::<Unit>("unit", "Units")?,
            ModelField::new::<Option<String>>("note", "Note")?,
            ModelField::new::<Vec<String>>("labels", "Labels")?,
            ModelField::new::<f64>("ratio", "Ratio")?,
        ])
    }
}

fn answer() -> Value {
    json!({"a/b~count":5,"unit":"mm","labels":["bolt","washer"],"ratio":0.5})
}

#[test]
fn handwritten_form_projects_rust_domains_effective_names_and_omission() {
    let form = TypedForm::<Answer>::new()
        .unwrap()
        .with_field_annotations("note", object(json!({"default":"Default note"})))
        .unwrap();
    assert_eq!(form.raw_form().as_json(), form.as_json());
    assert_eq!(
        form.as_json()["required"],
        json!(["a/b~count", "unit", "labels", "ratio"])
    );
    assert_eq!(
        form.as_json()["properties"]["a/b~count"],
        json!({"type":"integer","title":"Count","minimum":0,"maximum":255})
    );
    assert_eq!(
        form.as_json()["properties"]["unit"],
        json!({"type":"string","title":"Units","enum":["mm","in"]})
    );
    assert!(form.as_json()["properties"].get("count").is_none());
    let expected = Answer {
        count: 5,
        unit: Unit::Metric,
        note: None,
        labels: vec!["bolt".into(), "washer".into()],
        ratio: 0.5,
    };
    assert_eq!(
        form.decode_result(accept(answer())).unwrap(),
        TypedFormResult::Accept { content: expected }
    );
    // The default is retained on the schema but never inserted in accepted data.
    assert_eq!(
        form.as_json()["properties"]["note"]["default"],
        "Default note"
    );
}

#[test]
fn raw_validation_precedes_conversion_and_never_coerces_or_fills_defaults() {
    let form = TypedForm::<Answer>::new().unwrap();
    for (name, value) in [
        ("a/b~count", json!(null)),
        ("a/b~count", json!("5")),
        ("a/b~count", json!(1.5)),
        ("a/b~count", json!(-1)),
        ("a/b~count", json!(256)),
        ("unit", json!("centimeters")),
        ("note", json!(null)),
        ("labels", json!(["bolt", 5])),
        ("ratio", json!("0.5")),
    ] {
        let mut content = answer();
        content[name] = value;
        let raw = accept(content);
        assert!(form.raw_form().validate_result(&raw).is_err());
        let error = form.decode_result(raw).unwrap_err();
        assert!(matches!(error, Error::ModelValidation { .. }));
        let expected = if name == "a/b~count" {
            "/a~1b~0count".to_owned()
        } else if name == "labels" {
            "/labels/1".to_owned()
        } else {
            format!("/{name}")
        };
        assert_eq!(error.path(), Some(expected.as_str()));
    }
    let mut missing = object(answer());
    missing.remove("a/b~count");
    assert_eq!(
        form.decode_result(FormResult::Accept { content: missing })
            .unwrap_err()
            .path(),
        Some("/a~1b~0count")
    );
}

#[test]
fn typed_forms_reject_extra_fields_without_changing_raw_form_behavior() {
    let form = TypedForm::<Answer>::new().unwrap();
    let mut content = answer();
    content["extra/name~"] = json!("submitted string");
    let raw = accept(content);
    form.raw_form().validate_result(&raw).unwrap();
    let error = form.decode_result(raw).unwrap_err();
    assert!(matches!(error, Error::ModelValidation { .. }));
    assert_eq!(error.path(), Some("/extra~1name~0"));
    assert!(!error.to_string().contains("submitted string"));
}

#[derive(Debug, Deserialize, PartialEq)]
struct InconsistentAnswer {
    value: String,
}

impl Model for InconsistentAnswer {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![ModelField::new::<String>("different", "Different")?])
    }
}

#[test]
fn accept_converts_while_cancel_and_decline_bypass_an_inconsistent_model() {
    let form = TypedForm::<InconsistentAnswer>::new().unwrap();
    let error = form
        .decode_result(accept(json!({"different":"wire value"})))
        .unwrap_err();
    assert!(matches!(error, Error::ModelConversion { .. }));
    assert_eq!(error.path(), Some(""));
    assert_eq!(
        form.decode_result(FormResult::Cancel).unwrap(),
        TypedFormResult::Cancel
    );
    assert_eq!(
        form.decode_result(FormResult::Decline).unwrap(),
        TypedFormResult::Decline
    );
}

#[derive(Debug, Deserialize, PartialEq)]
struct ResourceAnswer {
    part: String,
    note: Option<String>,
    parts: Option<Vec<String>>,
    tags: Option<Vec<String>>,
}

impl Model for ResourceAnswer {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<String>("part", "Part")?,
            ModelField::new::<Option<String>>("note", "Note")?,
            ModelField::new::<Option<Vec<String>>>("parts", "Parts")?,
            ModelField::new::<Option<Vec<String>>>("tags", "Tags")?,
        ])
    }
}

#[test]
fn rich_runtime_annotations_preserve_resource_metadata_and_membership() {
    let input = json!({"type":"resource","options":[{
        "uri":"cad://parts/bolt","name":"bolt","title":"M6 bolt","mimeType":"model/stl",
        "_meta":{
            "openai/thumbnail":{"src":"https://example.com/bolt.png"},
            "openai/preview":{"target":{"type":"mcp_app_tool","name":"view_part","arguments":{"id":"bolt"}}},
            "example/keep":{"exact":true}
        }
    }]});
    let suggestions = json!([{
        "const":"Check joint","title":"Joint review","description":"Review fastener fit",
        "x-openai-thumbnail":{"src":"https://example.com/check.png"}
    }]);
    let form = TypedForm::<ResourceAnswer>::new()
        .unwrap()
        .with_field_annotations(
            "part",
            object(json!({"format":"uri","x-openai-input":input})),
        )
        .unwrap()
        .with_field_annotations(
            "note",
            object(json!({"description":"Reviewer note","x-openai-suggestions":suggestions})),
        )
        .unwrap()
        .with_field_annotations(
            "parts",
            object(json!({
                "items":{"format":"uri"},
                "x-openai-input":{"type":"resource","options":input["options"],"selection":"explicit"}
            })),
        )
        .unwrap()
        .with_field_annotations(
            "tags",
            object(json!({"items":{"x-openai-suggestions":suggestions}})),
        )
        .unwrap();
    assert_eq!(form.as_json()["required"], json!(["part"]));
    assert_eq!(
        form.as_json()["properties"]["part"]["x-openai-input"],
        input
    );
    assert_eq!(
        form.as_json()["properties"]["note"]["x-openai-suggestions"],
        suggestions
    );
    assert_eq!(
        form.as_json()["properties"]["parts"]["items"],
        json!({"type":"string","format":"uri"})
    );
    assert_eq!(
        form.as_json()["properties"]["tags"]["items"],
        json!({"type":"string","x-openai-suggestions":suggestions})
    );
    assert_eq!(
        form.decode_result(accept(json!({
            "part":"cad://parts/bolt","note":"Custom review",
            "parts":["cad://parts/bolt"],"tags":["Check joint","custom"]
        })))
        .unwrap(),
        TypedFormResult::Accept {
            content: ResourceAnswer {
                part: "cad://parts/bolt".into(),
                note: Some("Custom review".into()),
                parts: Some(vec!["cad://parts/bolt".into()]),
                tags: Some(vec!["Check joint".into(), "custom".into()]),
            }
        }
    );
    for (part, array_path) in [
        ("cad://parts/unknown", "/parts"),
        ("relative/path", "/parts/0"),
    ] {
        let error = form
            .decode_result(accept(json!({"part":part})))
            .unwrap_err();
        assert_eq!(error.path(), Some("/part"));
        assert!(!error.to_string().contains(part));
        assert_eq!(
            form.decode_result(accept(json!({"part":"cad://parts/bolt","parts":[part]})))
                .unwrap_err()
                .path(),
            Some(array_path)
        );
    }
    let request = FormInputRequired::new("opaque", "answer", "Choose a part", form.raw_form())
        .unwrap()
        .as_json();
    assert_eq!(
        request["inputRequests"]["answer"]["params"]["requestedSchema"],
        *form.as_json()
    );
}

#[test]
fn annotations_cannot_change_model_requiredness_type_or_domain() {
    for annotations in [
        json!({"type":"string"}),
        json!({"maximum":500}),
        json!({"default":256}),
        json!({"required":false}),
        json!({"nullable":true}),
    ] {
        assert!(
            TypedForm::<Answer>::new()
                .unwrap()
                .with_field_annotations("a/b~count", object(annotations))
                .is_err()
        );
    }
    assert!(
        TypedForm::<Answer>::new()
            .unwrap()
            .with_field_annotations("unit", object(json!({"enum":["mm","in","m"]})))
            .is_err()
    );
    let narrowed = TypedForm::<Answer>::new()
        .unwrap()
        .with_field_annotations("a/b~count", object(json!({"maximum":10})))
        .unwrap();
    let mut out_of_range = answer();
    out_of_range["a/b~count"] = json!(11);
    assert_eq!(
        narrowed
            .decode_result(accept(out_of_range))
            .unwrap_err()
            .path(),
        Some("/a~1b~0count")
    );
    let error = TypedForm::<Answer>::new()
        .unwrap()
        .with_field_annotations("missing/name~", JsonObject::new())
        .unwrap_err();
    assert_eq!(error.path(), Some("/missing~1name~0"));
    assert!(
        TypedForm::<ResourceAnswer>::new()
            .unwrap()
            .with_field_annotations("tags", object(json!({"items":{"type":"integer"}})))
            .is_err()
    );
}

#[test]
fn annotation_composition_replaces_enum_with_equivalent_titled_choices() {
    let choices = json!([
        {"const":"mm","title":"Metric","description":"Millimeters","x-openai-thumbnail":{"src":"https://example.com/metric.png"}},
        {"const":"in","title":"Imperial"}
    ]);
    let expected = json!({"type":"string","title":"Units","oneOf":choices});
    FormField::try_from(expected.clone()).unwrap();
    let form = TypedForm::<Answer>::new()
        .unwrap()
        .with_field_annotations("unit", object(json!({"enumNames":["Metric","Imperial"]})))
        .unwrap()
        .with_field_annotations("unit", object(json!({"oneOf":choices})))
        .unwrap();
    assert_eq!(form.as_json()["properties"]["unit"], expected);
    for unit in ["mm", "in"] {
        let mut content = answer();
        content["unit"] = json!(unit);
        form.raw_form()
            .validate_result(&accept(content.clone()))
            .unwrap();
        form.decode_result(accept(content)).unwrap();
    }
    let narrowed = form
        .with_field_annotations(
            "unit",
            object(json!({"oneOf":[{"const":"mm","title":"Metric only"}]})),
        )
        .unwrap();
    let mut incompatible = answer();
    incompatible["unit"] = json!("in");
    assert!(
        narrowed
            .raw_form()
            .validate_result(&accept(incompatible.clone()))
            .is_err()
    );
    assert_eq!(
        narrowed
            .decode_result(accept(incompatible))
            .unwrap_err()
            .path(),
        Some("/unit")
    );
    let legacy = narrowed
        .with_field_annotations("unit", object(json!({"enum":["mm"]})))
        .unwrap();
    assert_eq!(
        legacy.as_json()["properties"]["unit"],
        json!({"type":"string","title":"Units","enum":["mm"]})
    );
    legacy.decode_result(accept(answer())).unwrap();
    assert!(
        legacy
            .with_field_annotations(
                "unit",
                object(json!({"oneOf":[{"const":"m","title":"Meters"}]}))
            )
            .is_err()
    );
}

#[test]
fn annotation_composition_replaces_string_items_with_titled_choices() {
    let choices = json!([{"const":"bolt","title":"Bolt","x-openai-preview":{"src":"https://example.com/bolt.png"}},{"const":"washer","title":"Washer"}]);
    let expected = json!({"type":"array","title":"Labels","items":{"anyOf":choices}});
    FormField::try_from(expected.clone()).unwrap();
    let form = TypedForm::<Answer>::new()
        .unwrap()
        .with_field_annotations("labels", object(json!({"items":{"anyOf":choices}})))
        .unwrap();
    assert_eq!(form.as_json()["properties"]["labels"], expected);
    let form = form
        .with_field_annotations("labels", object(json!({"items":{}})))
        .unwrap();
    assert_eq!(form.as_json()["properties"]["labels"], expected);
    form.raw_form().validate_result(&accept(answer())).unwrap();
    form.decode_result(accept(answer())).unwrap();
    for invalid in [json!(["bolt", "spacer"]), json!(["bolt", 5])] {
        let mut content = answer();
        content["labels"] = invalid;
        assert!(
            form.raw_form()
                .validate_result(&accept(content.clone()))
                .is_err()
        );
        assert_eq!(
            form.decode_result(accept(content)).unwrap_err().path(),
            Some("/labels/1")
        );
    }
    let narrowed = form
        .with_field_annotations("labels", object(json!({"items":{"enum":["bolt"]}})))
        .unwrap();
    assert_eq!(
        narrowed.as_json()["properties"]["labels"]["items"],
        json!({"type":"string","enum":["bolt"]})
    );
    let mut content = answer();
    content["labels"] = json!(["bolt"]);
    narrowed.decode_result(accept(content)).unwrap();
    let titled = narrowed
        .with_field_annotations("labels", object(json!({"items":{"anyOf":[choices[0]]}})))
        .unwrap();
    assert_eq!(
        titled.as_json()["properties"]["labels"]["items"],
        json!({"anyOf":[choices[0]]})
    );
    assert!(
        titled
            .with_field_annotations(
                "labels",
                object(json!({"items":{"anyOf":[{"const":"spacer","title":"Spacer"}]}}))
            )
            .is_err()
    );
}

#[test]
fn annotation_composition_strengthens_existing_length_constraints() {
    let form = TypedForm::<Answer>::new()
        .unwrap()
        .with_field_annotations("note", object(json!({"minLength":2})))
        .unwrap()
        .with_field_annotations("note", object(json!({"minLength":3})))
        .unwrap()
        .with_field_annotations("note", object(json!({"maxLength":10})))
        .unwrap()
        .with_field_annotations("note", object(json!({"maxLength":5})))
        .unwrap();
    assert_eq!(
        form.as_json()["properties"]["note"],
        json!({"type":"string","title":"Note","minLength":3,"maxLength":5})
    );
    form.decode_result(accept(answer())).unwrap();
    let mut valid = answer();
    valid["note"] = json!("bolt");
    form.decode_result(accept(valid)).unwrap();
    for invalid in [json!("ab"), json!("abcdef"), json!(null)] {
        let mut content = answer();
        content["note"] = invalid;
        assert!(
            form.raw_form()
                .validate_result(&accept(content.clone()))
                .is_err()
        );
        assert_eq!(
            form.decode_result(accept(content)).unwrap_err().path(),
            Some("/note")
        );
    }
    assert!(
        form.with_field_annotations("note", object(json!({"minLength":2})))
            .is_err()
    );
}

#[test]
fn annotation_composition_preserves_item_constraints_and_legacy_labels() {
    let labeled = ModelField::new::<Unit>("unit", "Units")
        .unwrap()
        .with_annotations(object(json!({"enumNames":["Metric","Imperial"]})))
        .unwrap()
        .with_annotations(object(json!({"enum":["in","mm"]})))
        .unwrap();
    assert_eq!(
        labeled.field().as_json()["enumNames"],
        json!(["Imperial", "Metric"])
    );
    let narrowed = labeled
        .with_annotations(object(json!({"enum":["mm"]})))
        .unwrap();
    assert_eq!(narrowed.field().as_json()["enumNames"], json!(["Metric"]));

    for annotations in [
        json!({"items":{"anyOf":[{"const":"washer","title":"Washer"}]}}),
        json!({"items":{"anyOf":[{"const":"bo","title":"Too short"}]}}),
        json!({"items":{"anyOf":[{"const":5,"title":"Not a string"}]}}),
        json!({"items":{"anyOf":[{"const":"bolt","title":"Bolt"}],"type":"string"}}),
        json!({"items":{"anyOf":[{"const":"bolt","title":"Bolt","x-openai-preview":{"src":"file:///bolt.png"}}]}}),
    ] {
        let field = ModelField::new::<Vec<String>>("labels", "Labels")
            .unwrap()
            .with_annotations(object(json!({"items":{"minLength":3,"pattern":"^b"}})))
            .unwrap();
        assert!(field.with_annotations(object(annotations)).is_err());
    }
    let form = TypedForm::<Answer>::new()
        .unwrap()
        .with_field_annotations(
            "labels",
            object(json!({"items":{"minLength":3,"pattern":"^b"}})),
        )
        .unwrap()
        .with_field_annotations(
            "labels",
            object(json!({"items":{"anyOf":[{"const":"bolt","title":"Bolt"}]}})),
        )
        .unwrap();
    assert_eq!(
        form.as_json()["properties"]["labels"]["items"],
        json!({"anyOf":[{"const":"bolt","title":"Bolt"}]})
    );
    let mut content = answer();
    content["labels"] = json!(["bolt"]);
    form.decode_result(accept(content)).unwrap();
}

#[test]
fn annotation_composition_strengthens_array_cardinality_and_uniqueness() {
    let form = TypedForm::<Answer>::new()
        .unwrap()
        .with_field_annotations(
            "labels",
            object(json!({"minItems":1,"maxItems":4,"uniqueItems":false})),
        )
        .unwrap()
        .with_field_annotations(
            "labels",
            object(json!({"minItems":2,"maxItems":3,"uniqueItems":true})),
        )
        .unwrap();
    assert_eq!(
        form.as_json()["properties"]["labels"],
        json!({"type":"array","title":"Labels","items":{"type":"string"},"minItems":2,"maxItems":3,"uniqueItems":true})
    );
    form.decode_result(accept(answer())).unwrap();
    for labels in [
        json!(["bolt"]),
        json!(["bolt", "bolt"]),
        json!(["bolt", "washer", "spacer", "nut"]),
    ] {
        let mut content = answer();
        content["labels"] = labels;
        assert_eq!(
            form.decode_result(accept(content)).unwrap_err().path(),
            Some("/labels")
        );
    }
    for annotations in [
        json!({"minItems":1}),
        json!({"maxItems":5}),
        json!({"uniqueItems":false}),
    ] {
        let field = ModelField::new::<Vec<String>>("labels", "Labels")
            .unwrap()
            .with_annotations(object(
                json!({"minItems":2,"maxItems":3,"uniqueItems":true}),
            ))
            .unwrap();
        assert!(field.with_annotations(object(annotations)).is_err());
    }
}

#[derive(Debug, Deserialize)]
struct NumericAnswer {
    large: u64,
    signed: i64,
    ratio: f64,
}

impl Model for NumericAnswer {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<u64>("large", "Large")?,
            ModelField::new::<i64>("signed", "Signed")?,
            ModelField::new::<f64>("ratio", "Ratio")?,
        ])
    }
}

#[test]
fn integral_wire_representations_decode_to_the_same_rust_integer() {
    let form = TypedForm::<Answer>::new().unwrap();
    for spelling in ["1", "1.0", "1e0", "-0.0"] {
        let mut content = answer();
        content["a/b~count"] = serde_json::from_str(spelling).unwrap();
        form.raw_form()
            .validate_result(&accept(content.clone()))
            .unwrap();
        let TypedFormResult::Accept { content } = form.decode_result(accept(content)).unwrap()
        else {
            panic!("accepted answer expected")
        };
        assert_eq!(content.count, if spelling == "-0.0" { 0 } else { 1 });
    }
}

#[test]
fn integer_normalization_preserves_exact_values_and_rejects_overflow() {
    let form = TypedForm::<NumericAnswer>::new().unwrap();
    for (large, signed, expected_large, expected_signed) in [
        (json!(u64::MAX), json!(i64::MAX), u64::MAX, i64::MAX),
        (
            json!(9_007_199_254_740_993_u64),
            json!(i64::MIN),
            9_007_199_254_740_993,
            i64::MIN,
        ),
        (
            json!(18_446_744_073_709_549_568.0),
            json!(-9_223_372_036_854_775_808.0),
            18_446_744_073_709_549_568,
            i64::MIN,
        ),
        (
            json!(0.0),
            json!(9_223_372_036_854_774_784.0),
            0,
            9_223_372_036_854_774_784,
        ),
    ] {
        let TypedFormResult::Accept { content } = form
            .decode_result(accept(json!({"large":large,"signed":signed,"ratio":0})))
            .unwrap()
        else {
            panic!("accepted answer expected")
        };
        assert_eq!(content.large, expected_large);
        assert_eq!(content.signed, expected_signed);
    }
    for (name, number) in [
        ("large", json!(18_446_744_073_709_551_616.0)),
        ("large", json!(-1.0)),
        ("signed", json!(9_223_372_036_854_775_808.0)),
        ("signed", json!(-9_223_372_036_854_777_856.0)),
    ] {
        let mut content = json!({"large":0,"signed":0,"ratio":0});
        content[name] = number;
        assert_eq!(
            form.decode_result(accept(content)).unwrap_err().path(),
            Some(format!("/{name}").as_str())
        );
    }
}

#[derive(Debug, Deserialize)]
struct NumberAnswer<T> {
    value: T,
}
impl<T: FieldType> Model for NumberAnswer<T> {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![ModelField::new::<T>("value", "Value")?])
    }
}
fn decode_number<T: FieldType + DeserializeOwned>(
    form: &TypedForm<NumberAnswer<T>>,
    value: Value,
) -> T {
    let TypedFormResult::Accept { content } =
        form.decode_result(accept(json!({"value":value}))).unwrap()
    else {
        panic!("accepted answer expected")
    };
    content.value
}

#[test]
fn f32_rounding_must_preserve_the_annotated_domain() {
    for minimum in [1e-46, 1.000_000_000_000_001] {
        let form = TypedForm::<NumberAnswer<f32>>::new()
            .unwrap()
            .with_field_annotations("value", object(json!({"minimum":minimum})))
            .unwrap();
        let submitted = accept(json!({"value":minimum}));
        form.raw_form().validate_result(&submitted).unwrap();
        assert_eq!(
            form.decode_result(submitted).unwrap_err().path(),
            Some("/value")
        );
        assert_eq!(decode_number(&form, json!(2)), 2.0);
    }
    let form = TypedForm::<NumberAnswer<f32>>::new().unwrap();
    assert_eq!(decode_number(&form, json!(1e-46)), 0.0);
    assert_eq!(decode_number(&form, json!(0.1)), 0.1_f32);
    assert_eq!(decode_number(&form, json!(f32::MAX)), f32::MAX);
    // At this midpoint, converting through f64 would round twice and differ
    // from Serde's direct u64-to-f32 conversion.
    let integer = 9_223_372_586_610_589_697_u64;
    assert_ne!(integer as f32, (integer as f64) as f32);
    assert_eq!(decode_number(&form, json!(integer)), integer as f32);
}

#[test]
fn f64_rounding_of_integer_inputs_must_preserve_the_annotated_domain() {
    for (bound, integer) in [
        ("minimum", 9_007_199_254_740_993_u64),
        ("maximum", u64::MAX),
    ] {
        let annotations = JsonObject::from_iter([(bound.into(), json!(integer))]);
        let form = TypedForm::<NumberAnswer<f64>>::new()
            .unwrap()
            .with_field_annotations("value", annotations)
            .unwrap();
        let submitted = accept(json!({"value":integer}));
        form.raw_form().validate_result(&submitted).unwrap();
        assert_eq!(
            form.decode_result(submitted).unwrap_err().path(),
            Some("/value")
        );
    }
}

#[test]
fn annotation_composition_retains_exact_integer_bounds_and_number_domains() {
    let maximum = u64::MAX - 1;
    let minimum = i64::MIN + 1;
    let form = TypedForm::<NumericAnswer>::new()
        .unwrap()
        .with_field_annotations("large", object(json!({"minimum":maximum - 1,"maximum":maximum})))
        .unwrap()
        .with_field_annotations("large", object(json!({"minimum":maximum})))
        .unwrap()
        .with_field_annotations("signed", object(json!({"minimum":minimum})))
        .unwrap()
        .with_field_annotations("ratio", object(json!({"minimum":0,"maximum":10,"multipleOf":2})))
        .unwrap()
        .with_field_annotations("ratio", object(json!({"minimum":1,"maximum":9,"exclusiveMinimum":2,"exclusiveMaximum":9,"multipleOf":4})))
        .unwrap()
        .with_field_annotations("ratio", object(json!({"exclusiveMinimum":3,"exclusiveMaximum":8})))
        .unwrap();
    assert_eq!(form.as_json()["properties"]["large"]["type"], "integer");
    assert_eq!(form.as_json()["properties"]["ratio"]["type"], "number");
    let content = json!({"large":maximum,"signed":minimum,"ratio":4});
    let TypedFormResult::Accept { content: decoded } =
        form.decode_result(accept(content.clone())).unwrap()
    else {
        panic!("accepted result")
    };
    assert_eq!(decoded.large, maximum);
    assert_eq!(decoded.signed, minimum);
    assert_eq!(decoded.ratio, 4.0);
    for (name, invalid) in [
        ("large", json!(maximum - 1)),
        ("large", json!(u64::MAX)),
        ("large", json!(1.5)),
        ("signed", json!(i64::MIN)),
        ("ratio", json!(8)),
        ("ratio", json!(6)),
        ("ratio", json!("4")),
    ] {
        let mut content = content.clone();
        content[name] = invalid;
        assert!(
            form.raw_form()
                .validate_result(&accept(content.clone()))
                .is_err()
        );
        assert_eq!(
            form.decode_result(accept(content)).unwrap_err().path(),
            Some(format!("/{name}").as_str())
        );
    }
    for (name, annotations) in [
        ("large", json!({"maximum":u64::MAX})),
        ("large", json!({"minimum":maximum - 1})),
        ("signed", json!({"minimum":i64::MIN})),
        ("ratio", json!({"minimum":0})),
        ("ratio", json!({"maximum":10})),
        ("ratio", json!({"exclusiveMinimum":2})),
        ("ratio", json!({"exclusiveMaximum":9})),
        ("ratio", json!({"multipleOf":2})),
    ] {
        let base = match name {
            "large" => ModelField::new::<u64>(name, name).unwrap().with_annotations(object(json!({"minimum":maximum,"maximum":maximum}))),
            "signed" => ModelField::new::<i64>(name, name).unwrap().with_annotations(object(json!({"minimum":minimum}))),
            _ => ModelField::new::<f64>(name, name).unwrap().with_annotations(object(json!({"minimum":1,"maximum":9,"exclusiveMinimum":3,"exclusiveMaximum":8,"multipleOf":4}))),
        }.unwrap();
        assert!(base.with_annotations(object(annotations)).is_err());
    }
    assert!(
        ModelField::new::<u8>("count", "Count")
            .unwrap()
            .with_annotations(object(json!({"type":"number"})))
            .is_err()
    );
    assert!(
        ModelField::new::<f32>("ratio", "Ratio")
            .unwrap()
            .with_annotations(object(json!({"maximum":f64::MAX})))
            .is_err()
    );
}

#[test]
fn annotation_composition_keeps_final_schema_validation() {
    let input = json!({"type":"resource","options":[{
        "uri":"cad://parts/bolt","name":"bolt",
        "_meta":{"openai/thumbnail":{"src":"https://example.com/bolt.png"},"openai/preview":{"target":{"type":"mcp_app_tool","name":"view_part","arguments":{"id":"bolt"}}}}
    }]});
    for (field, annotations) in [
        (
            "part",
            json!({"format":"uri","oneOf":[{"const":"cad://parts/bolt","title":"Bolt"}],"x-openai-input":input}),
        ),
        (
            "parts",
            json!({"items":{"anyOf":[{"const":"cad://parts/bolt","title":"Bolt"}]},"x-openai-input":input}),
        ),
    ] {
        assert!(
            TypedForm::<ResourceAnswer>::new()
                .unwrap()
                .with_field_annotations(field, object(annotations))
                .is_err()
        );
    }
    let unit = ModelField::new::<Unit>("unit", "Unit")
        .unwrap()
        .with_annotations(object(json!({"default":"mm"})))
        .unwrap();
    assert!(
        unit.with_annotations(object(json!({"oneOf":[{"const":"in","title":"Imperial"}]})))
            .is_err()
    );
    for annotations in [
        json!({"oneOf":[{"const":"bolt","title":"Bolt","x-openai-thumbnail":{"src":"file:///bolt.png"}}]}),
        json!({"oneOf":[{"const":"bolt","title":"Bolt"}],"enum":["bolt"]}),
        json!({"minLength":null}),
        json!({"pattern":"^m"}),
        json!({"additionalProperties":false}),
    ] {
        let field = ModelField::new::<String>("note", "Note")
            .unwrap()
            .with_annotations(object(json!({"minLength":2,"pattern":"^b"})))
            .unwrap();
        assert!(field.with_annotations(object(annotations)).is_err());
    }
}

fn params(result: FormResult) -> CallToolRequestParams {
    serde_json::from_value(json!({
        "name":"review","requestState":"  opaque state  ",
        "inputResponses":{"answer":result,"another":{"action":"cancel"}}
    }))
    .unwrap()
}

#[test]
fn mrtr_retains_raw_continuation_checks_state_and_typed_decoding() {
    let form = TypedForm::<Answer>::new().unwrap();
    assert_eq!(
        form.read_input(&CallToolRequestParams::new("review"), "answer")
            .unwrap(),
        TypedFormInput::Initial
    );
    for result in [accept(answer()), FormResult::Cancel, FormResult::Decline] {
        let request = params(result.clone());
        let expected = form.decode_result(result).unwrap();
        assert_eq!(
            form.read_input(&request, "answer").unwrap(),
            TypedFormInput::Response {
                request_state: "  opaque state  ".into(),
                result: expected,
            }
        );
        assert_eq!(request.input_responses.as_ref().unwrap().len(), 2);
    }
    for request in [
        json!({"name":"review","requestState":"state"}),
        json!({"name":"review","inputResponses":{"answer":{"action":"cancel"}}}),
        json!({"name":"review","requestState":"state","inputResponses":{"other":{"action":"cancel"}}}),
        json!({"name":"review","requestState":"state","inputResponses":{"answer":{"action":"unknown"}}}),
    ] {
        let request = serde_json::from_value(request).unwrap();
        assert!(form.read_input(&request, "answer").is_err());
    }
    let mut invalid_count = answer();
    invalid_count["a/b~count"] = json!(256);
    let request = params(accept(invalid_count));
    assert_eq!(
        form.read_input(&request, "answer").unwrap_err().path(),
        Some("/a~1b~0count")
    );
    assert!(matches!(
        form.read_input(&request, " "),
        Err(Error::Validation(_))
    ));
    let mut content = answer();
    content["extra"] = json!("Keep raw acceptance");
    let request = params(accept(content));
    assert!(matches!(
        openai_mcp_extensions::forms::mrtr::read_input(&request, "answer", form.raw_form())
            .unwrap(),
        FormInput::Response { .. }
    ));
    assert_eq!(
        form.read_input(&request, "answer").unwrap_err().path(),
        Some("/extra")
    );
}

#[derive(Deserialize)]
struct NonSendAnswer {
    #[allow(dead_code)]
    name: String,
    #[serde(skip)]
    marker: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Model for NonSendAnswer {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![ModelField::new::<String>("name", "Name")?])
    }
}

#[test]
fn form_description_does_not_own_or_require_a_send_answer() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<TypedForm<NonSendAnswer>>();
    TypedForm::<NonSendAnswer>::new().unwrap();
}

struct DirectServer {
    form: TypedForm<Answer>,
}

impl ServerHandler for DirectServer {
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Owned(vec![ProtocolVersion::V_2025_11_25])
    }

    async fn call_tool(
        &self,
        _: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = self
            .form
            .elicit_input(&context, "Review the parts")
            .await
            .map_err(ErrorData::from)?;
        let content = match result {
            TypedFormResult::Accept { content } => json!({
                "count":content.count,"metric":content.unit == Unit::Metric,"note":content.note
            }),
            TypedFormResult::Cancel => json!({"action":"cancel"}),
            TypedFormResult::Decline => json!({"action":"decline"}),
        };
        Ok(CallToolResult::structured(content).into())
    }
}

#[tokio::test]
async fn legacy_direct_wrapper_sends_the_raw_schema_and_uses_typed_decoding() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let form = TypedForm::<Answer>::new().unwrap();
        let expected_schema = form.as_json().clone();
        let (server_io, client_io) = tokio::io::duplex(65536);
        let server = tokio::spawn(DirectServer { form }.serve(server_io));
        let mut rpc = wire::Rpc::new(client_io);
        rpc.request(
            1,
            "initialize",
            json!({
                "protocolVersion":"2025-11-25",
                "capabilities":{"extensions":{"openai/elicitation":{"form":{}}}},
                "clientInfo":{"name":"typed-host","version":"1"}
            }),
        )
        .await;
        rpc.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        let server = server.await.unwrap().unwrap();
        rpc.legacy_answer = Some(json!({"action":"accept","content":answer()}));
        let response = rpc.request(2, "tools/call", json!({"name":"review"})).await;
        assert_eq!(
            response["result"]["structuredContent"],
            json!({"count":5,"metric":true,"note":null})
        );
        assert_eq!(rpc.legacy_requests.len(), 1);
        assert_eq!(
            rpc.legacy_requests[0]["params"]["requestedSchema"],
            expected_schema
        );
        let mut extra = answer();
        extra["extra"] = json!(true);
        rpc.legacy_answer = Some(json!({"action":"accept","content":extra}));
        let invalid = rpc.request(3, "tools/call", json!({"name":"review"})).await;
        assert!(invalid.get("error").is_some());
        assert!(
            invalid["error"]["message"]
                .as_str()
                .unwrap()
                .contains("/extra")
        );
        server.cancel().await.unwrap();
    })
    .await
    .unwrap();
}
