#[cfg(feature = "mcp-apps")]
use openai_mcp_extensions::ui::{
    APP_MIME_TYPE, app_html_resource, associate_app_resource, supports_mcp_apps,
};
use openai_mcp_extensions::{
    Error,
    forms::{FormResult, OpenAIForm, PreviewTarget},
    mentions::{MentionItem, MentionResource, MentionSearchResult},
    resources::{
        FileEntrypointInput, ResourceRepresentation, requested_representation,
        resource_tool_call_metadata,
    },
    settings::{SettingsGroup, SettingsLayoutItem, SettingsRegistration},
    ui::{DisplayMode, Entrypoint, UiResourceMetadata, UiToolMetadata},
};
use rmcp::model::{MetaObject, RequestMetaObject};
use serde_json::{Value, json};

fn accept(content: Value) -> FormResult {
    serde_json::from_value(json!({"action":"accept","content":content})).unwrap()
}

#[test]
fn forms_preserve_spec_annotations_and_validate_actual_answers() {
    let wire = json!({"type":"object","properties":{
        "code":{"type":"string","pattern":"^[A-Z]{2}$","x-openai-suggestions":[{"const":"AB","title":"Example","description":"Optional hint","x-openai-thumbnail":{"src":"https://example.com/a.png"}}]},
        "parts":{"type":"array","minItems":1,"uniqueItems":true,"items":{"anyOf":[{"const":"bolt","title":"Bolt"},{"const":"nut","title":"Nut"}]}},
        "count":{"type":"integer","minimum":1,"maximum":3},
        "enabled":{"type":"boolean","default":true}
    },"required":["code","parts","count"]});
    let form = OpenAIForm::from_json(wire.clone()).unwrap();
    assert_eq!(form.as_json(), &wire);
    let answer = accept(json!({"code":"AB","parts":["bolt"],"count":2}));
    form.validate_result(&answer).unwrap();
    assert_eq!(
        serde_json::to_value(answer).unwrap()["content"].get("enabled"),
        None
    );
    for bad in [
        json!({"code":"a","parts":["bolt"],"count":2}),
        json!({"code":"AB","parts":["bolt","bolt"],"count":2}),
        json!({"code":"AB","parts":["other"],"count":2}),
        json!({"code":"AB","parts":["bolt"],"count":1.5}),
        json!({"parts":["bolt"],"count":2}),
    ] {
        assert!(form.validate_result(&accept(bad)).is_err());
    }
    form.validate_result(&FormResult::Cancel).unwrap();
    form.validate_result(&FormResult::Decline).unwrap();
}

#[test]
fn resource_selection_has_membership_and_default_rules() {
    let field = json!({"type":"array","items":{"type":"string","format":"uri"},"x-openai-input":{"type":"resource","options":[{"uri":"file:///bolt.stl","name":"bolt.stl","_meta":{"openai/thumbnail":{"src":"https://example.com/bolt.png"},"openai/preview":{"target":{"type":"mcp_app_tool","name":"preview","arguments":{}}}}}]}});
    let make = |field| {
        OpenAIForm::from_json(
            json!({"type":"object","properties":{"files":field},"required":["files"]}),
        )
    };
    let form = make(field.clone()).unwrap();
    form.validate_result(&accept(json!({"files":["file:///bolt.stl"]})))
        .unwrap();
    assert!(
        form.validate_result(&accept(json!({"files":["file:///other.stl"]})))
            .is_err()
    );
    assert!(
        form.validate_result(&accept(json!({"files":["not a URI"]})))
            .is_err()
    );
    let mut user = field.clone();
    user["x-openai-input"]["userOptions"] = json!({});
    make(user)
        .unwrap()
        .validate_result(&accept(json!({"files":["file:///other.stl"]})))
        .unwrap();
    let mut implicit = field.clone();
    implicit["x-openai-input"]["selection"] = json!("implicit");
    make(implicit.clone())
        .unwrap()
        .validate_result(&accept(json!({"files":["file:///other.stl"]})))
        .unwrap();
    implicit["default"] = json!(["file:///bolt.stl"]);
    assert!(make(implicit).is_err());
    let mut defaults = field.clone();
    defaults["default"] = json!(["file:///other.stl"]);
    assert!(make(defaults).is_err());
    let single = json!({"type":"string","format":"uri","x-openai-input":{"type":"file","options":[],"selection":"explicit"}});
    assert!(make(single).is_err());
    let wrong = json!({"type":"string","x-openai-input":{"type":"future_input","options":[]}});
    assert!(make(wrong).is_err());
    let mut bad_thumbnail = field;
    bad_thumbnail["x-openai-input"]["options"][0]["_meta"]["openai/thumbnail"] = json!({"src":42});
    assert!(make(bad_thumbnail).is_err());
}

#[test]
fn unsupported_or_malformed_forms_are_rejected_before_sending() {
    for field in [
        json!({"type":"object"}),
        json!({"type":"string","$ref":"https://example.com/schema"}),
        json!({"type":"array","items":{"type":"number"}}),
        json!({"type":"string","pattern":"["}),
        json!({"type":"string","minLength":-1}),
        json!({"type":"integer","default":"wrong"}),
        json!({"type":"string","oneOf":[{"const":1}]}),
        json!({"type":"string","format":"made-up"}),
    ] {
        assert!(
            OpenAIForm::from_json(json!({"type":"object","properties":{"value":field}})).is_err()
        );
    }
    assert!(
        OpenAIForm::from_json(json!({"type":"object","properties":{},"required":["missing"]}))
            .is_err()
    );
}

#[test]
fn titled_form_options_require_string_titles_before_construction() {
    for option in [
        json!({"const":"bolt"}),
        json!({"const":"bolt","title":null}),
        json!({"const":"bolt","title":42}),
        json!({"const":"bolt","title":false}),
        json!({"const":"bolt","title":[]}),
        json!({"const":"bolt","title":{}}),
    ] {
        for field in [
            json!({"type":"string","oneOf":[option]}),
            json!({"type":"array","items":{"anyOf":[option]}}),
            json!({"type":"string","x-openai-suggestions":[option]}),
            json!({"type":"array","items":{"type":"string","x-openai-suggestions":[option]}}),
        ] {
            let error = OpenAIForm::from_json(json!({
                "type":"object","properties":{"part":field}
            }))
            .unwrap_err();
            assert!(matches!(error, Error::Configuration { .. }));
            assert!(error.to_string().contains("option.title must be a string"));
        }
    }
}

#[test]
fn titled_form_options_preserve_titles_and_optional_descriptions() {
    for option in [
        json!({"const":"bolt","title":"Bolt"}),
        json!({"const":"bolt","title":"Bolt","description":"A fastener"}),
        json!({"const":"bolt","title":""}),
    ] {
        for field in [
            json!({"type":"string","oneOf":[option]}),
            json!({"type":"array","items":{"anyOf":[option]}}),
            json!({"type":"string","x-openai-suggestions":[option]}),
            json!({"type":"array","items":{"type":"string","x-openai-suggestions":[option]}}),
        ] {
            let wire = json!({"type":"object","properties":{"part":field}});
            let form = OpenAIForm::from_json(wire.clone()).unwrap();
            assert_eq!(form.as_json(), &wire);
        }
    }
    assert!(
        OpenAIForm::from_json(json!({"type":"object","properties":{
            "part":{"type":"string","oneOf":[{"const":"bolt","title":"Bolt","description":42}]}
        }}))
        .is_err()
    );
}

#[test]
fn untitled_form_enums_remain_supported() {
    let wire = json!({"type":"object","properties":{
        "part":{"type":"string","enum":["bolt","nut"]},
        "parts":{"type":"array","items":{"type":"string","enum":["bolt","nut"]}}
    },"required":["part","parts"]});
    let form = OpenAIForm::from_json(wire.clone()).unwrap();
    assert_eq!(form.as_json(), &wire);
    form.validate_result(&accept(json!({"part":"bolt","parts":["nut"]})))
        .unwrap();
    assert!(
        form.validate_result(&accept(json!({"part":"washer","parts":["nut"]})))
            .is_err()
    );
    assert!(
        form.validate_result(&accept(json!({"part":"bolt","parts":["washer"]})))
            .is_err()
    );
}

#[test]
fn form_thumbnails_require_https_or_base64_image_data() {
    let schema = |src| {
        json!({"type":"object","properties":{
            "part":{"type":"string","oneOf":[{"const":"bolt","title":"Bolt",
                "x-openai-thumbnail":{"src":src}}]}
        }})
    };
    for src in [
        "http://example.com/bolt.png",
        "file:///bolt.png",
        "https://",
        "data:text/plain;base64,Ym9sdA==",
        "data:image/png;base64,%%%",
        "data:image/png;base64,",
    ] {
        assert!(
            OpenAIForm::from_json(schema(src)).is_err(),
            "accepted {src}"
        );
    }
    for src in [
        "https://example.com/bolt.png",
        "data:image/svg+xml;base64,PHN2Zy8+",
    ] {
        let wire = schema(src);
        assert_eq!(
            OpenAIForm::from_json(wire.clone()).unwrap().as_json(),
            &wire
        );
    }
}

#[test]
fn settings_validate_schema_complete_values_patch_and_layout() {
    let schema = json!({"type":"object","properties":{"mode":{"type":"string","title":"Mode","enum":["a","b"]},"count":{"type":"integer","title":"Count","minimum":1}},"required":["mode"]});
    let settings = SettingsRegistration::new(schema.clone()).unwrap();
    settings
        .validate_values(json!({"mode":"a","count":1}).as_object().unwrap())
        .unwrap();
    for bad in [
        json!({"mode":"a"}),
        json!({"mode":"a","count":0}),
        json!({"mode":"a","count":1,"extra":false}),
    ] {
        assert!(settings.validate_values(bad.as_object().unwrap()).is_err());
    }
    settings
        .validate_patch(json!({"count":2}).as_object().unwrap())
        .unwrap();
    for bad in [json!({}), json!({"mode":"c"}), json!({"extra":2})] {
        assert!(settings.validate_patch(bad.as_object().unwrap()).is_err());
    }
    let group = |property: &str| SettingsGroup::Group {
        title: "General".into(),
        items: vec![SettingsLayoutItem::Property {
            property: property.into(),
        }],
    };
    assert!(
        SettingsRegistration::new(schema.clone())
            .unwrap()
            .with_layout(vec![group("mode"), group("mode")])
            .is_err()
    );
    assert!(
        SettingsRegistration::new(schema.clone())
            .unwrap()
            .with_layout(vec![group("missing")])
            .is_err()
    );
    assert!(
        SettingsRegistration::new(schema)
            .unwrap()
            .with_tool_names("same", "same")
            .is_err()
    );
    for field in [
        json!({"type":"string","title":"  "}),
        json!({"type":"string","title":"Value","default":"a"}),
        json!({"type":"object","title":"Value"}),
        json!({"type":"number","title":"Value","enum":["a"]}),
        json!({"type":"string","title":"Value","oneOf":[]}),
    ] {
        assert!(
            SettingsRegistration::new(json!({"type":"object","properties":{"value":field}}))
                .is_err()
        );
    }
}

#[test]
fn metadata_merges_without_losing_other_namespaces() {
    let mut meta = MetaObject(
        json!({"ui":{"resourceUri":"ui://viewer"},"other":{"keep":true}})
            .as_object()
            .unwrap()
            .clone(),
    );
    UiToolMetadata {
        entrypoints: Some(vec![Entrypoint::File {
            extensions: vec![" .stl ".into()],
        }]),
        ..Default::default()
    }
    .apply(&mut meta)
    .unwrap();
    assert_eq!(meta.0["ui"], json!({"resourceUri":"ui://viewer"}));
    assert_eq!(
        meta.0["openai/ui"]["entrypoints"][0],
        json!({"type":"file","extensions":[".stl"]})
    );
    UiResourceMetadata {
        available_display_modes: Some(vec![DisplayMode::Inline, DisplayMode::Fullscreen]),
        preferred_display_mode: Some(DisplayMode::Fullscreen),
    }
    .apply(&mut meta)
    .unwrap();
    assert_eq!(
        meta.0["openai/ui"],
        json!({"availableDisplayModes":["inline","fullscreen"],"preferredDisplayMode":"fullscreen"})
    );
    assert_eq!(meta.0["other"], json!({"keep":true}));
    let before = meta.clone();
    let error = UiToolMetadata {
        entrypoints: Some(vec![Entrypoint::File {
            extensions: vec!["stl".into()],
        }]),
        ..Default::default()
    }
    .apply(&mut meta)
    .unwrap_err();
    assert!(matches!(error, Error::Configuration { .. }));
    assert_eq!(meta, before);
}

#[test]
fn openai_display_modes_reject_unsupported_wire_values_before_application() {
    for mode in [DisplayMode::Inline, DisplayMode::Fullscreen] {
        let wire = match mode {
            DisplayMode::Inline => "inline",
            DisplayMode::Fullscreen => "fullscreen",
        };
        assert_eq!(serde_json::to_value(mode).unwrap(), json!(wire));
        assert_eq!(
            serde_json::from_value::<DisplayMode>(json!(wire)).unwrap(),
            mode
        );
    }
    assert!(serde_json::from_value::<DisplayMode>(json!("pip")).is_err());
    for wire in [
        json!({"availableDisplayModes":["inline","pip"]}),
        json!({"preferredDisplayMode":"pip"}),
    ] {
        assert!(serde_json::from_value::<UiResourceMetadata>(wire).is_err());
    }
    assert!(
        serde_json::from_value::<UiToolMetadata>(json!({"preferredModelDisplayMode":"pip"}))
            .is_err()
    );
}

#[test]
fn resource_helpers_distinguish_missing_and_malformed_metadata() {
    let meta: RequestMetaObject =
        serde_json::from_value(json!({"openai/resource":{"path":"/host/bolt.stl"}})).unwrap();
    assert_eq!(
        resource_tool_call_metadata(&meta).unwrap().unwrap().path,
        "/host/bolt.stl"
    );
    assert!(
        resource_tool_call_metadata(&RequestMetaObject::default())
            .unwrap()
            .is_none()
    );
    let bad = serde_json::from_value(json!({"openai/resource":{"path":5}})).unwrap();
    assert!(resource_tool_call_metadata(&bad).is_err());
    let file: FileEntrypointInput = serde_json::from_value(
        json!({"file":{"name":"bolt.stl","resourceUri":"file:///bolt.stl"}}),
    )
    .unwrap();
    file.validate().unwrap();

    for (wire, expected) in [
        (json!({}), None),
        (json!({"openai/resource":{"path":"/host/file"}}), None),
        (
            json!({"openai/resource":{"representation":"text","extra":true}}),
            Some(ResourceRepresentation::Text),
        ),
        (
            json!({"openai/resource":{"representation":"blob"}}),
            Some(ResourceRepresentation::Blob),
        ),
    ] {
        let meta = serde_json::from_value(wire).unwrap();
        assert_eq!(requested_representation(&meta).unwrap(), expected);
    }
    for wire in [
        json!({"openai/resource":true}),
        json!({"openai/resource":{"representation":"binary"}}),
        json!({"openai/resource":{"representation":null}}),
    ] {
        assert!(requested_representation(&serde_json::from_value(wire).unwrap()).is_err());
    }
}

#[test]
fn resource_previews_validate_the_spec_union_without_stripping_metadata() {
    let schema = |target| {
        json!({"type":"object","properties":{
            "part":{"type":"string","format":"uri","x-openai-input":{"type":"resource",
                "options":[{"uri":"cad://parts/bolt","name":"Bolt","_meta":{
                    "openai/preview":{"target":target},"example/other":{"keep":true}
                }}]}}
        }})
    };
    for target in [
        json!({"type":"mcp_app_tool","name":"view_part"}),
        json!({"type":"mcp_app_tool","name":"view_part","arguments":{"id":"bolt"}}),
        json!({"type":"resource_link","uri":"cad://parts/bolt","name":"Bolt","_meta":{"extra":true}}),
    ] {
        let wire = schema(target.clone());
        assert_eq!(
            OpenAIForm::from_json(wire.clone()).unwrap().as_json(),
            &wire
        );
        let target: PreviewTarget = serde_json::from_value(target).unwrap();
        let mut meta = MetaObject(json!({"other":true}).as_object().unwrap().clone());
        target.apply(&mut meta).unwrap();
        assert_eq!(meta.0["other"], true);
        assert_eq!(
            meta.0["openai/preview"]["target"],
            serde_json::to_value(target).unwrap()
        );
    }
    for target in [
        json!({"type":"mcp_app_tool","name":" "}),
        json!({"type":"mcp_app_tool","name":"view","arguments":[]}),
        json!({"type":"mcp_app_tool","name":"view","arguments":null}),
        json!({"type":"resource_link","uri":"relative/path","name":"Bolt"}),
        json!({"type":"resource_link","uri":"cad://parts/bolt"}),
        json!({"type":"future_target","name":"view"}),
    ] {
        assert!(OpenAIForm::from_json(schema(target)).is_err());
    }
}

#[test]
#[cfg(feature = "mcp-apps")]
fn mcp_app_helpers_preserve_security_and_visibility_declarations() {
    let mut meta = MetaObject(
        json!({"ui":{"visibility":["app"]},"other":true})
            .as_object()
            .unwrap()
            .clone(),
    );
    associate_app_resource(&mut meta, "ui://parts/viewer").unwrap();
    let associated = meta.clone();
    associate_app_resource(&mut meta, "ui://parts/viewer").unwrap();
    assert_eq!(meta, associated);
    assert_eq!(
        meta.0["ui"],
        json!({"visibility":["app"],"resourceUri":"ui://parts/viewer"})
    );
    assert_eq!(meta.0["other"], true);
    let before = meta.clone();
    // Two SDKs must not silently redirect a tool to different UI resources.
    let error = associate_app_resource(&mut meta, "ui://parts/other").unwrap_err();
    assert!(matches!(
        error,
        Error::Configuration { source }
            if matches!(*source, Error::McpAppsMetadata(mcp_apps_server::MetadataError::Conflict { .. }))
    ));
    assert_eq!(meta, before);
    for uri in ["https://example.com/app", "ui://", "ui://parts/ bad"] {
        assert!(associate_app_resource(&mut meta, uri).is_err());
        assert_eq!(meta, before);
    }
    let mut bad = MetaObject(json!({"ui":true}).as_object().unwrap().clone());
    assert!(associate_app_resource(&mut bad, "ui://parts/viewer").is_err());
    assert_eq!(bad.0["ui"], true);
    let security = json!({"ui":{"csp":{"connectDomains":[],"resourceDomains":[]}},"openai/ui":{"preferredDisplayMode":"inline"}});
    let contents = app_html_resource(
        "ui://parts/viewer",
        "<main>Parts</main>",
        MetaObject(security.as_object().unwrap().clone()),
    )
    .unwrap();
    let wire = serde_json::to_value(contents).unwrap();
    assert_eq!(wire["mimeType"], APP_MIME_TYPE);
    assert_eq!(wire["_meta"], security);
    for (capabilities, supported) in [
        (json!({}), false),
        (
            json!({"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":[APP_MIME_TYPE]}}}),
            true,
        ),
        (
            json!({"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":["text/plain"]}}}),
            false,
        ),
        (
            json!({"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":APP_MIME_TYPE}}}),
            false,
        ),
        (
            json!({"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":[APP_MIME_TYPE, 1]}}}),
            false,
        ),
    ] {
        assert_eq!(
            supports_mcp_apps(&serde_json::from_value(capabilities).unwrap()),
            supported
        );
    }
}

#[test]
#[cfg(feature = "mcp-apps")]
fn app_association_rejects_conflicting_legacy_alias_without_mutation() {
    let mut meta = MetaObject(
        json!({"ui/resourceUri":"ui://parts/other","openai/ui":{"entrypoints":[{"type":"thread"}]}})
            .as_object().unwrap().clone(),
    );
    let before = meta.clone();
    assert!(associate_app_resource(&mut meta, "ui://parts/viewer").is_err());
    assert_eq!(meta, before);
}

#[test]
fn mention_variants_have_exact_protocol_tags() {
    let resource =
        serde_json::from_value(json!({"uri":"file:///bolt.stl","name":"bolt.stl"})).unwrap();
    let result = MentionSearchResult {
        items: vec![
            MentionItem::ResourceLink(resource),
            MentionItem::Resource(MentionResource {
                resource_uri: "file:///nut.stl".into(),
                title: "Nut".into(),
                subtitle: None,
                icons: None,
            }),
        ],
    };
    result.validate().unwrap();
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["items"][0]["type"], "resource_link");
    assert_eq!(
        value["items"][1],
        json!({"type":"resource","resourceUri":"file:///nut.stl","title":"Nut"})
    );
    assert_eq!(
        serde_json::from_value::<MentionSearchResult>(value).unwrap(),
        result
    );
}

#[test]
fn unusual_property_names_survive_validation_and_serialization() {
    let form = OpenAIForm::from_json(json!({"type":"object","properties":{"__proto__":{"type":"string"},"constructor":{"type":"boolean"},"a/b~c":{"type":"integer"}},"required":["__proto__","constructor","a/b~c"]})).unwrap();
    form.validate_result(&accept(
        json!({"__proto__":"ok","constructor":true,"a/b~c":2}),
    ))
    .unwrap();
    assert!(
        form.validate_result(&accept(json!({"constructor":true,"a/b~c":2})))
            .is_err()
    );
}
