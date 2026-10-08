#![cfg(feature = "derive")]

extern crate openai_mcp_extensions as renamed_sdk;

use openai_mcp_extensions::{
    Error, Model, SettingsModel, StringEnum,
    forms::{FormResult, TypedForm, TypedFormResult},
    model::{FieldType, ModelField},
    settings::TypedSettings,
};
use rmcp::model::JsonObject;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn object(value: Value) -> JsonObject {
    value.as_object().expect("object").clone()
}

#[derive(Debug, PartialEq, Serialize, Deserialize, StringEnum)]
#[serde(rename_all = "camelCase")]
enum Units {
    #[serde(rename = "mm")]
    Millimetres,
    ImperialInches,
}

#[derive(Debug, PartialEq, Deserialize, Model)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Answer {
    #[openai(
        title = "Part",
        description = "Part name",
        min_length = 1,
        max_length = 12
    )]
    part_name: String,
    #[serde(rename = "a/b~c")]
    #[openai(title = "Count", minimum = 1, maximum = 100)]
    count: u8,
    #[openai(title = "Units")]
    units: Units,
    #[openai(title = "Note")]
    #[serde(default)]
    note: Option<String>,
    #[openai(title = "Labels")]
    labels: Vec<String>,
    r#type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManualAnswer {
    part_name: String,
    #[serde(rename = "a/b~c")]
    count: u8,
    units: Units,
    note: Option<String>,
    labels: Vec<String>,
    r#type: String,
}

impl Model for ManualAnswer {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<String>("partName", "Part")?.with_annotations(object(
                json!({"description":"Part name","minLength":1,"maxLength":12}),
            ))?,
            ModelField::new::<u8>("a/b~c", "Count")?
                .with_annotations(object(json!({"minimum":1,"maximum":100})))?,
            ModelField::new::<Units>("units", "Units")?,
            ModelField::new::<Option<String>>("note", "Note")?,
            ModelField::new::<Vec<String>>("labels", "Labels")?,
            ModelField::new::<String>("type", "type")?,
        ])
    }
}

fn valid_answer() -> Value {
    json!({"partName":"Bolt","a/b~c":3,"units":"mm","labels":["metal"],"type":"fastener"})
}

#[test]
fn derived_models_share_native_numeric_conversion() {
    #[derive(Debug, Deserialize, Model)]
    struct FloatAnswer {
        #[openai(minimum = 1e-46)]
        value: f32,
    }
    #[derive(Serialize, SettingsModel)]
    struct FloatSettings {
        #[openai(minimum = 1e-46)]
        value: f32,
    }
    let form = TypedForm::<Answer>::new().unwrap();
    let settings = TypedSettings::<Preferences>::new().unwrap();
    for spelling in ["1.0", "1e0"] {
        let number: Value = serde_json::from_str(spelling).unwrap();
        let mut answer = valid_answer();
        answer["a/b~c"] = number.clone();
        let TypedFormResult::Accept { content } = form
            .decode_result(FormResult::Accept {
                content: object(answer),
            })
            .unwrap()
        else {
            panic!("accepted answer expected")
        };
        assert_eq!(content.count, 1);
        let patch = settings
            .decode_patch(object(json!({"count":number})))
            .unwrap();
        assert_eq!(patch.count, Some(1));
        assert_eq!(patch.show_grid, None);
    }
    let form = TypedForm::<FloatAnswer>::new().unwrap();
    assert_eq!(
        form.decode_result(FormResult::Accept {
            content: object(json!({"value":1e-46}))
        })
        .unwrap_err()
        .path(),
        Some("/value")
    );
    assert_eq!(
        TypedSettings::<FloatSettings>::new()
            .unwrap()
            .decode_patch(object(json!({"value":1e-46})))
            .err()
            .unwrap()
            .path(),
        Some("/value")
    );
    let TypedFormResult::Accept { content } = form
        .decode_result(FormResult::Accept {
            content: object(json!({"value":1})),
        })
        .unwrap()
    else {
        panic!("accepted answer expected")
    };
    assert_eq!(content.value, 1.0);
}

#[test]
fn derived_and_handwritten_models_have_identical_native_behavior() {
    let derived = TypedForm::<Answer>::new().unwrap();
    let manual = TypedForm::<ManualAnswer>::new().unwrap();
    assert_eq!(derived.as_json(), manual.as_json());
    assert_eq!(
        derived.as_json()["required"],
        json!(["partName", "a/b~c", "units", "labels", "type"])
    );
    assert_eq!(derived.as_json()["properties"]["a/b~c"]["minimum"], 1);
    assert_eq!(derived.as_json()["properties"]["a/b~c"]["maximum"], 100);
    let TypedFormResult::Accept {
        content: derived_answer,
    } = derived
        .decode_result(FormResult::Accept {
            content: object(valid_answer()),
        })
        .unwrap()
    else {
        panic!("accepted answer expected")
    };
    let TypedFormResult::Accept {
        content: manual_answer,
    } = manual
        .decode_result(FormResult::Accept {
            content: object(valid_answer()),
        })
        .unwrap()
    else {
        panic!("accepted answer expected")
    };
    assert_eq!(derived_answer.part_name, manual_answer.part_name);
    assert_eq!(derived_answer.count, manual_answer.count);
    assert_eq!(derived_answer.units, manual_answer.units);
    assert_eq!(derived_answer.note, manual_answer.note);
    assert_eq!(derived_answer.labels, manual_answer.labels);
    assert_eq!(derived_answer.r#type, manual_answer.r#type);
    assert_eq!(derived_answer.note, None);

    for (name, value) in [
        ("partName", json!("")),
        ("a/b~c", json!(0)),
        ("a/b~c", json!(256)),
        ("units", json!("Millimetres")),
        ("note", Value::Null),
        ("labels", json!([3])),
        ("unknown", json!(true)),
    ] {
        let mut answer = object(valid_answer());
        answer.insert(name.into(), value);
        assert!(
            derived
                .decode_result(FormResult::Accept { content: answer })
                .is_err()
        );
    }
}

#[test]
fn string_enum_schema_uses_actual_serde_values() {
    assert_eq!(
        Units::schema().unwrap(),
        json!({"type":"string","enum":["mm","imperialInches"]})
    );
    for unit in [Units::Millimetres, Units::ImperialInches] {
        let value = serde_json::to_value(&unit).unwrap();
        assert_eq!(serde_json::from_value::<Units>(value).unwrap(), unit);
    }

    #[derive(Serialize, StringEnum)]
    enum Duplicate {
        #[serde(rename = "same")]
        First,
        #[serde(rename = "same")]
        Second,
    }
    assert!(Duplicate::schema().is_err());

    #[derive(StringEnum)]
    enum NonString {
        Unit,
    }
    impl Serialize for NonString {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_bool(true)
        }
    }
    assert!(NonString::schema().is_err());
}

#[derive(Debug, PartialEq, Serialize, Deserialize, SettingsModel)]
#[serde(rename_all = "camelCase")]
#[openai(patch_serialize)]
struct Preferences {
    #[openai(title = "Show grid")]
    show_grid: bool,
    #[serde(rename = "display/caption~text")]
    #[openai(title = "Caption")]
    caption: String,
    #[openai(title = "Count", maximum = 100)]
    count: u8,
    #[openai(title = "Units")]
    units: Units,
}

#[test]
fn generated_patch_preserves_omission_false_zero_empty_and_renames() {
    let settings = TypedSettings::<Preferences>::new().unwrap();
    let values = Preferences {
        show_grid: true,
        caption: "Current".into(),
        count: 7,
        units: Units::Millimetres,
    };
    let set = object(json!({"showGrid":false,"display/caption~text":"","count":0}));
    let patch: PreferencesPatch = settings.decode_patch(set.clone()).unwrap();
    assert_eq!(patch.show_grid, Some(false));
    assert_eq!(patch.caption, Some(String::new()));
    assert_eq!(patch.count, Some(0));
    assert_eq!(patch.units, None);
    assert_eq!(serde_json::to_value(&patch).unwrap(), Value::Object(set));
    assert_eq!(
        serde_json::to_value(PreferencesPatch::default()).unwrap(),
        json!({})
    );
    assert_eq!(
        settings.encode_values(&values).unwrap(),
        object(json!({
            "showGrid":true,"display/caption~text":"Current","count":7,"units":"mm"
        }))
    );

    let complete = Preferences::fields().unwrap();
    let partial = PreferencesPatch::fields().unwrap();
    for (value, patch) in complete.iter().zip(&partial) {
        assert_eq!(value.name(), patch.name());
        assert_eq!(value.field().as_json(), patch.field().as_json());
        assert!(value.is_required());
        assert!(!patch.is_required());
    }
    for invalid in [
        json!({}),
        json!({"showGrid":null}),
        json!({"unknown":true}),
        json!({"count":101}),
        json!({"units":"other"}),
    ] {
        assert!(settings.decode_patch(object(invalid)).is_err());
    }
}

#[test]
fn optional_aliases_are_rejected_in_complete_settings() {
    type OptionalString = Option<String>;
    #[derive(Serialize, Deserialize, SettingsModel)]
    struct Invalid {
        value: OptionalString,
    }
    let error = Invalid::fields().unwrap_err();
    assert_eq!(error.path(), Some("/value"));
    assert!(TypedSettings::<Invalid>::new().is_err());
}

#[derive(Serialize, Deserialize, SettingsModel)]
#[openai(crate = "renamed_sdk", patch_serialize)]
struct RenamedDependency {
    enabled: bool,
}

#[test]
fn generated_patch_uses_the_configured_sdk_and_serde_paths() {
    let settings = TypedSettings::<RenamedDependency>::new().unwrap();
    let patch = settings
        .decode_patch(object(json!({"enabled":false})))
        .unwrap();
    assert_eq!(patch.enabled, Some(false));
    assert_eq!(
        serde_json::to_value(patch).unwrap(),
        json!({"enabled":false})
    );
}

#[test]
fn pattern_and_multiple_constraints_use_native_validation_in_forms_and_settings() {
    #[derive(Deserialize, Model)]
    struct ConstrainedAnswer {
        #[openai(pattern = "^[A-Z][a-z]+$")]
        part: String,
        #[openai(minimum = 0, maximum = 100, multiple_of = 2)]
        count: u8,
    }
    #[derive(Serialize, SettingsModel)]
    struct ConstrainedSettings {
        #[openai(pattern = "^[A-Z][a-z]+$")]
        part: String,
        #[openai(minimum = 0, maximum = 100, multiple_of = 2)]
        count: u8,
    }
    let form = TypedForm::<ConstrainedAnswer>::new().unwrap();
    let settings = TypedSettings::<ConstrainedSettings>::new().unwrap();
    let native = [
        ModelField::new::<String>("part", "part")
            .unwrap()
            .with_annotations(object(json!({"pattern":"^[A-Z][a-z]+$"})))
            .unwrap(),
        ModelField::new::<u8>("count", "count")
            .unwrap()
            .with_annotations(object(json!({"minimum":0,"maximum":100,"multipleOf":2})))
            .unwrap(),
    ];
    let complete = ConstrainedSettings::fields().unwrap();
    let patch = ConstrainedSettingsPatch::fields().unwrap();
    for ((native, complete), patch) in native.iter().zip(&complete).zip(&patch) {
        assert_eq!(native.field().as_json(), complete.field().as_json());
        assert_eq!(complete.field().as_json(), patch.field().as_json());
    }
    let TypedFormResult::Accept { content } = form
        .decode_result(FormResult::Accept {
            content: object(json!({"part":"Bolt","count":4})),
        })
        .unwrap()
    else {
        panic!("accepted answer expected");
    };
    assert_eq!((content.part.as_str(), content.count), ("Bolt", 4));
    let decoded = settings
        .decode_patch(object(json!({"part":"Bolt","count":4})))
        .unwrap();
    assert_eq!(
        (decoded.part.as_deref(), decoded.count),
        (Some("Bolt"), Some(4))
    );
    assert!(
        settings
            .encode_values(&ConstrainedSettings {
                part: "Bolt".into(),
                count: 4
            })
            .is_ok()
    );
    for (value, pointer) in [
        (json!({"part":"bolt","count":4}), "/part"),
        (json!({"part":"Bolt","count":3}), "/count"),
    ] {
        let error = form
            .decode_result(FormResult::Accept {
                content: object(value.clone()),
            })
            .err()
            .unwrap();
        assert_eq!(error.path(), Some(pointer));
        let error = settings.decode_patch(object(value)).err().unwrap();
        assert_eq!(error.path(), Some(pointer));
    }

    #[derive(Model)]
    struct InvalidMultiple {
        #[openai(multiple_of = 0)]
        _count: u8,
    }
    #[derive(Model)]
    struct InvalidPattern {
        #[openai(pattern = "[")]
        _part: String,
    }
    #[derive(Model)]
    struct IncompatibleConstraint {
        #[openai(pattern = "literal")]
        _count: u8,
    }
    assert!(InvalidMultiple::fields().is_err());
    assert!(InvalidPattern::fields().is_err());
    assert!(IncompatibleConstraint::fields().is_err());
}

#[derive(Debug, Deserialize, Model)]
struct GenericAnswer<T>
where
    T: PartialEq,
{
    value: T,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, PartialEq, Deserialize)]
struct DecodeOnly(String);

impl FieldType for DecodeOnly {
    fn schema() -> Result<Value, Error> {
        String::schema()
    }
}

#[test]
fn generic_forms_preserve_domains_defaults_and_deserialization_only_fields() {
    let numbers = TypedForm::<GenericAnswer<u8>>::new().unwrap();
    assert_eq!(numbers.as_json()["properties"]["value"]["maximum"], 255);
    let TypedFormResult::Accept { content } = numbers
        .decode_result(FormResult::Accept {
            content: object(json!({"value":0})),
        })
        .unwrap()
    else {
        panic!("accepted answer expected");
    };
    assert_eq!(content.value, 0);
    assert_eq!(content.note, None);
    for value in [json!({"value":256}), json!({"value":0,"note":null})] {
        assert!(
            numbers
                .decode_result(FormResult::Accept {
                    content: object(value)
                })
                .is_err()
        );
    }
    let strings = TypedForm::<GenericAnswer<String>>::new().unwrap();
    assert_eq!(strings.as_json()["properties"]["value"]["type"], "string");
    let decode_only = TypedForm::<GenericAnswer<DecodeOnly>>::new().unwrap();
    let TypedFormResult::Accept { content } = decode_only
        .decode_result(FormResult::Accept {
            content: object(json!({"value":"owned","note":"text"})),
        })
        .unwrap()
    else {
        panic!("accepted answer expected");
    };
    assert_eq!(content.value, DecodeOnly("owned".into()));
    assert_eq!(content.note.as_deref(), Some("text"));
    assert!(TypedForm::<GenericAnswer<Option<Option<String>>>>::new().is_err());
}

#[derive(Serialize, SettingsModel)]
#[serde(rename_all = "camelCase")]
#[openai(crate = "renamed_sdk", patch_serialize)]
struct GenericPreferences<T>
where
    T: PartialEq,
{
    item_value: T,
    enabled: bool,
}

#[derive(PartialEq, Serialize, Deserialize)]
struct NoDefault(String);

impl FieldType for NoDefault {
    fn schema() -> Result<Value, Error> {
        String::schema()
    }
}

#[derive(PartialEq, Serialize)]
struct EncodeOnly(String);

impl FieldType for EncodeOnly {
    fn schema() -> Result<Value, Error> {
        String::schema()
    }
}

#[test]
fn generic_settings_preserve_one_way_traits_and_patch_default_without_field_defaults() {
    let settings = TypedSettings::<GenericPreferences<NoDefault>>::new().unwrap();
    let defaults = GenericPreferencesPatch::<NoDefault>::default();
    assert!(defaults.item_value.is_none());
    assert!(defaults.enabled.is_none());
    assert_eq!(serde_json::to_value(defaults).unwrap(), json!({}));
    let patch = settings
        .decode_patch(object(json!({"itemValue":"replacement"})))
        .unwrap();
    assert_eq!(patch.item_value.unwrap().0, "replacement");
    assert_eq!(patch.enabled, None);
    assert_eq!(
        settings
            .encode_values(&GenericPreferences {
                item_value: NoDefault("current".into()),
                enabled: false,
            })
            .unwrap(),
        object(json!({"itemValue":"current","enabled":false}))
    );

    let integer_settings = TypedSettings::<GenericPreferences<u16>>::new().unwrap();
    assert_eq!(
        integer_settings
            .decode_patch(object(json!({"itemValue":0})))
            .unwrap()
            .item_value,
        Some(0)
    );
    assert!(
        integer_settings
            .decode_patch(object(json!({"itemValue":65536})))
            .is_err()
    );
    // Merely describing values needs no decoding trait; describing a patch
    // needs no encoding trait. TypedSettings requires the actual read/update pair.
    assert!(GenericPreferences::<EncodeOnly>::fields().is_ok());
    assert!(GenericPreferences::<Option<String>>::fields().is_err());
    assert!(GenericPreferencesPatch::<DecodeOnly>::fields().is_ok());
    let decode_only_patch: GenericPreferencesPatch<DecodeOnly> =
        serde_json::from_value(json!({"itemValue":"decoded"})).unwrap();
    assert_eq!(decode_only_patch.item_value.unwrap().0, "decoded");

    // This complete model has a custom whole-model serializer; its field only
    // decodes. An unused patch serializer must not impose Serialize on the field.
    #[derive(SettingsModel)]
    struct ManuallyEncodedSettings {
        value: DecodeOnly,
    }
    impl Serialize for ManuallyEncodedSettings {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;
            let mut fields = serializer.serialize_struct("ManuallyEncodedSettings", 1)?;
            fields.serialize_field("value", &self.value.0)?;
            fields.end()
        }
    }
    let settings = TypedSettings::<ManuallyEncodedSettings>::new().unwrap();
    assert_eq!(
        settings
            .encode_values(&ManuallyEncodedSettings {
                value: DecodeOnly("current".into())
            })
            .unwrap(),
        object(json!({"value":"current"}))
    );
    let patch = settings
        .decode_patch(object(json!({"value":"replacement"})))
        .unwrap();
    assert_eq!(patch.value.unwrap().0, "replacement");

    trait SelectedValue {
        type Value;
    }
    struct Selector;
    impl SelectedValue for Selector {
        type Value = String;
    }
    #[derive(SettingsModel)]
    #[openai(patch_serialize)]
    struct SelectedPreferences<S>
    where
        S: SelectedValue,
        S::Value: serde::de::DeserializeOwned,
    {
        value: S::Value,
    }
    impl<S> Serialize for SelectedPreferences<S>
    where
        S: SelectedValue,
        S::Value: Serialize + serde::de::DeserializeOwned,
    {
        fn serialize<R: serde::Serializer>(&self, serializer: R) -> Result<R::Ok, R::Error> {
            use serde::ser::SerializeStruct;
            let mut fields = serializer.serialize_struct("SelectedPreferences", 1)?;
            fields.serialize_field("value", &self.value)?;
            fields.end()
        }
    }
    // The owner parameter does not implement any Serde or Default trait.
    // Bounds belong to its actual associated field type, not to S itself.
    let selected = TypedSettings::<SelectedPreferences<Selector>>::new().unwrap();
    let defaults = SelectedPreferencesPatch::<Selector>::default();
    assert_eq!(serde_json::to_value(defaults).unwrap(), json!({}));
    let selected_patch = selected
        .decode_patch(object(json!({"value":"chosen"})))
        .unwrap();
    assert_eq!(selected_patch.value.as_deref(), Some("chosen"));
    assert_eq!(
        serde_json::to_value(selected_patch).unwrap(),
        json!({"value":"chosen"})
    );
}
