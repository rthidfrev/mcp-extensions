use openai_mcp_extensions::{
    Error,
    forms::{FormField, FormResult, OpenAIForm, ResourceInput, ResourceSelection},
    settings::{SettingsGroup, SettingsLayoutItem, SettingsRegistration},
    ui::{
        DisplayMode, Entrypoint, ModelDisplayMode, OpenAIUiResourceExt, OpenAIUiToolExt,
        UiResourceMetadata, UiToolMetadata,
    },
};
use rmcp::model::{MetaObject, Resource, ResourceContents, Tool};
use serde_json::{Value, json};

fn meta(value: Value) -> MetaObject {
    serde_json::from_value(value).unwrap()
}

fn accept(content: Value) -> FormResult {
    serde_json::from_value(json!({"action":"accept","content":content})).unwrap()
}

#[test]
fn form_constructors_preserve_the_spec_wire_shapes() {
    let choices = [("bolt", "M6 bolt"), ("washer", "M6 washer")];
    for (field, expected) in [
        (
            FormField::string_array("Parts"),
            json!({"type":"array","title":"Parts","items":{"type":"string"}}),
        ),
        (
            FormField::string_enum("Units", ["mm", "in"]).unwrap(),
            json!({"type":"string","title":"Units","enum":["mm","in"]}),
        ),
        (
            FormField::string_choices("Part", choices).unwrap(),
            json!({"type":"string","title":"Part","oneOf":[
                {"const":"bolt","title":"M6 bolt"},
                {"const":"washer","title":"M6 washer"}
            ]}),
        ),
        (
            FormField::string_suggestions("Part", choices).unwrap(),
            json!({"type":"string","title":"Part","x-openai-suggestions":[
                {"const":"bolt","title":"M6 bolt"},
                {"const":"washer","title":"M6 washer"}
            ]}),
        ),
        (
            FormField::string_array_suggestions("Parts", choices).unwrap(),
            json!({"type":"array","title":"Parts","items":{
                "type":"string","x-openai-suggestions":[
                    {"const":"bolt","title":"M6 bolt"},
                    {"const":"washer","title":"M6 washer"}
                ]
            }}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&field).unwrap(), expected);
        assert_eq!(field.as_json(), &expected);
    }
    assert!(FormField::string_enum("Units", Vec::<String>::new()).is_err());
    assert!(FormField::string_choices("Part", Vec::<(String, String)>::new()).is_err());
}

#[test]
fn suggested_values_remain_open_while_choices_restrict_answers() {
    let form = OpenAIForm::new(
        [
            (
                "choice".into(),
                FormField::string_choices("Part", [("bolt", "Bolt")]).unwrap(),
            ),
            (
                "suggestion".into(),
                FormField::string_suggestions("Part", [("bolt", "Bolt")]).unwrap(),
            ),
            (
                "suggestions".into(),
                FormField::string_array_suggestions("Parts", [("bolt", "Bolt")]).unwrap(),
            ),
        ],
        vec!["choice".into()],
    )
    .unwrap();
    let answer = accept(json!({
        "choice":"bolt","suggestion":"custom-part","suggestions":["bolt","custom-washer"]
    }));
    form.validate_result(&answer).unwrap();
    assert_eq!(
        serde_json::to_value(answer).unwrap()["content"]["suggestions"],
        json!(["bolt", "custom-washer"])
    );
    assert!(
        form.validate_result(&accept(json!({"choice":"custom-part"})))
            .is_err()
    );
}

#[test]
fn single_and_multiple_resources_preserve_native_resource_options() {
    let resource = Resource::new("cad://parts/bolt", "bolt")
        .with_title("M6 bolt")
        .with_description("Library part")
        .with_mime_type("model/stl")
        .with_size(42)
        .with_meta(meta(json!({
            "example/other":{"keep":true},
            "openai/thumbnail":{"src":"https://example.com/bolt.png"},
            "openai/preview":{"target":{"type":"mcp_app_tool","name":"preview_part"}}
        })));
    let option = serde_json::to_value(&resource).unwrap();
    let input = ResourceInput::new(vec![resource]);
    let single = FormField::single_resource("Part", input.clone()).unwrap();
    let mut multiple_input = input.clone();
    multiple_input.selection = Some(ResourceSelection::Explicit);
    let multiple = FormField::multiple_resources("Parts", multiple_input.clone()).unwrap();
    assert_eq!(single.as_json()["type"], "string");
    assert_eq!(single.as_json()["format"], "uri");
    assert_eq!(multiple.as_json()["type"], "array");
    assert_eq!(
        multiple.as_json()["items"],
        json!({"type":"string","format":"uri"})
    );
    for field in [&single, &multiple] {
        assert_eq!(field.as_json()["x-openai-input"]["options"][0], option);
        assert_eq!(field.as_json()["x-openai-input"]["type"], "resource");
    }
    assert_eq!(
        multiple.as_json()["x-openai-input"]["selection"],
        "explicit"
    );
    assert!(FormField::single_resource("Part", multiple_input).is_err());

    let bad = ResourceInput::new(vec![Resource::new("relative/path", "part")]);
    assert!(FormField::multiple_resources("Parts", bad).is_err());

    let form = OpenAIForm::new(
        [("single".into(), single), ("multiple".into(), multiple)],
        vec![],
    )
    .unwrap();
    form.validate_result(&accept(json!({
        "single":"cad://parts/bolt","multiple":["cad://parts/bolt"]
    })))
    .unwrap();
    for answer in [
        json!({"single":["cad://parts/bolt"]}),
        json!({"multiple":"cad://parts/bolt"}),
        json!({"single":"cad://parts/unknown"}),
    ] {
        assert!(form.validate_result(&accept(answer)).is_err());
    }
}

#[test]
fn resource_picker_constructors_express_existing_user_options() {
    let options = vec![Resource::new("file:///bolt.stl", "bolt.stl")];
    let files = ResourceInput::files(options.clone(), Some(vec![".stl".into()]));
    let directories = ResourceInput::directories(options.clone());
    assert_eq!(
        serde_json::to_value(&files).unwrap(),
        json!({
            "type":"resource","options":options,
            "userOptions":{"kind":"file","accept":[".stl"]}
        })
    );
    assert_eq!(
        serde_json::to_value(&directories).unwrap(),
        json!({"type":"resource","options":options,"userOptions":{"kind":"directory"}})
    );
    assert_eq!(
        serde_json::to_value(ResourceInput::files(vec![], None)).unwrap(),
        json!({"type":"resource","options":[],"userOptions":{"kind":"file"}})
    );
    FormField::single_resource("File", files).unwrap();
    FormField::multiple_resources("Directories", directories).unwrap();
}

fn settings() -> SettingsRegistration {
    SettingsRegistration::new(json!({"type":"object","properties":{
        "display/show-grid":{"type":"boolean","title":"Show grid"},
        "units":{"type":"string","title":"Units","enum":["mm","in"]}
    }}))
    .unwrap()
}

#[test]
fn settings_layout_constructors_preserve_order_and_registration_validation() {
    let group = SettingsGroup::new(
        "Display",
        [
            SettingsLayoutItem::property("display/show-grid"),
            SettingsLayoutItem::tool("view_parts", "Browse parts"),
            SettingsLayoutItem::property("units"),
        ],
    );
    assert_eq!(
        serde_json::to_value(&group).unwrap(),
        json!({"kind":"group","title":"Display","items":[
            {"kind":"property","property":"display/show-grid"},
            {"kind":"tool","tool":"view_parts","title":"Browse parts"},
            {"kind":"property","property":"units"}
        ]})
    );
    settings().with_layout(vec![group]).unwrap();
    for layout in [
        vec![SettingsGroup::new(
            "Display",
            [SettingsLayoutItem::property("unknown")],
        )],
        vec![
            SettingsGroup::new("Display", [SettingsLayoutItem::property("units")]),
            SettingsGroup::new("Other", [SettingsLayoutItem::property("units")]),
        ],
        vec![SettingsGroup::new(" ", [])],
        vec![SettingsGroup::new(
            "Display",
            [SettingsLayoutItem::tool(" ", "Browse parts")],
        )],
        vec![SettingsGroup::new(
            "Display",
            [SettingsLayoutItem::tool("view_parts", " ")],
        )],
    ] {
        assert!(settings().with_layout(layout).is_err());
    }
}

fn tool() -> Tool {
    serde_json::from_value(json!({
        "name":"view_parts","title":"Parts tray","description":"Open parts",
        "inputSchema":{"type":"object","properties":{}},
        "outputSchema":{"type":"object","properties":{"opened":{"type":"boolean"}}},
        "annotations":{"readOnlyHint":true},
        "icons":[{"src":"https://example.com/icon.png","mimeType":"image/png"}],
        "_meta":{
            "ui":{"visibility":["app"],"example/hint":true},
            "example/other":{"keep":true},"openai/ui":{"preferredModelDisplayMode":"inline"}
        }
    }))
    .unwrap()
}

#[test]
fn native_tool_ui_helpers_preserve_fields_and_other_metadata() {
    let mut tool = tool();
    let mut expected = serde_json::to_value(&tool).unwrap();
    tool.apply_openai_ui(&UiToolMetadata {
        entrypoints: Some(vec![Entrypoint::File {
            extensions: vec![" .stl ".into()],
        }]),
        preferred_model_display_mode: Some(ModelDisplayMode::Fullscreen),
    })
    .unwrap();
    expected["_meta"]["openai/ui"] = json!({
        "entrypoints":[{"type":"file","extensions":[".stl"]}],
        "preferredModelDisplayMode":"fullscreen"
    });
    assert_eq!(serde_json::to_value(tool).unwrap(), expected);
}

#[test]
fn native_tool_ui_failures_leave_the_entire_original_unchanged() {
    for mut tool in [
        tool(),
        Tool::new(
            "empty",
            "No metadata",
            json!({"type":"object"}).as_object().unwrap().clone(),
        ),
    ] {
        let before = tool.clone();
        assert!(matches!(
            tool.apply_openai_ui(&UiToolMetadata {
                entrypoints: Some(vec![Entrypoint::File {
                    extensions: vec!["stl".into()],
                }]),
                ..Default::default()
            })
            .unwrap_err(),
            Error::Configuration { .. }
        ));
        assert_eq!(tool, before);
    }
}

#[cfg(feature = "mcp-apps")]
#[test]
fn native_app_association_preserves_fields_and_is_atomic_on_failure() {
    let mut tool_with_ui = tool();
    let mut expected = serde_json::to_value(&tool_with_ui).unwrap();
    tool_with_ui
        .associate_app_resource("ui://parts/viewer")
        .unwrap();
    expected["_meta"]["ui"]["resourceUri"] = json!("ui://parts/viewer");
    assert_eq!(serde_json::to_value(&tool_with_ui).unwrap(), expected);
    for mut tool in [
        tool_with_ui,
        Tool::new(
            "empty",
            "No metadata",
            json!({"type":"object"}).as_object().unwrap().clone(),
        ),
    ] {
        let before = tool.clone();
        assert!(matches!(
            tool.associate_app_resource("https://example.com/app")
                .unwrap_err(),
            Error::Configuration { .. }
        ));
        assert_eq!(tool, before);
    }
    let mut malformed = tool();
    malformed.meta.as_mut().unwrap().0["ui"] = json!(true);
    let before = malformed.clone();
    assert!(matches!(
        malformed
            .associate_app_resource("ui://parts/viewer")
            .unwrap_err(),
        Error::Configuration { .. }
    ));
    assert_eq!(malformed, before);
}

#[test]
fn native_resource_ui_helpers_preserve_text_blob_and_security_metadata() {
    let metadata = meta(json!({
        "ui":{"csp":{"connectDomains":[],"resourceDomains":[]}},
        "example/other":{"keep":true},"openai/ui":{"preferredDisplayMode":"inline"}
    }));
    for mut contents in [
        ResourceContents::text("<main>Parts</main>", "ui://parts/viewer")
            .with_mime_type("text/html;profile=mcp-app")
            .with_meta(metadata.clone()),
        ResourceContents::blob("AAEC", "file:///parts.bin")
            .with_mime_type("application/octet-stream")
            .with_meta(metadata),
    ] {
        let mut expected = serde_json::to_value(&contents).unwrap();
        contents
            .apply_openai_ui(&UiResourceMetadata {
                available_display_modes: Some(vec![DisplayMode::Inline, DisplayMode::Fullscreen]),
                preferred_display_mode: Some(DisplayMode::Fullscreen),
            })
            .unwrap();
        expected["_meta"]["openai/ui"] = json!({
            "availableDisplayModes":["inline","fullscreen"],"preferredDisplayMode":"fullscreen"
        });
        assert_eq!(serde_json::to_value(&contents).unwrap(), expected);
    }
    let mut bare = ResourceContents::text("Original", "file:///original.txt");
    let mut expected = serde_json::to_value(&bare).unwrap();
    bare.apply_openai_ui(&UiResourceMetadata::default())
        .unwrap();
    expected["_meta"] = json!({"openai/ui":{}});
    assert_eq!(serde_json::to_value(bare).unwrap(), expected);
}
