use super::{expand_enum, expand_model};
use syn::parse_quote;

#[test]
fn structural_errors_explain_the_supported_contract() {
    for (input, expected) in [
        (parse_quote! { struct Tuple(String); }, "named fields"),
        (parse_quote! { enum Wrong { A } }, "named-field struct"),
        (
            parse_quote! { struct Borrowed<'a> { item: &'a str } },
            "lifetime parameters",
        ),
        (
            parse_quote! { struct Optional { item: ::std::option::Option<String> } },
            "complete settings",
        ),
        (
            parse_quote! { #[derive(Model, SettingsModel)] struct Both { item: String } },
            "already implements Model",
        ),
    ] {
        let error = expand_model(&input, true).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
    for (input, expected) in [
        (parse_quote! { struct Wrong; }, "requires an enum"),
        (parse_quote! { enum Empty {} }, "at least one variant"),
        (
            parse_quote! { enum Data { Value(String) } },
            "unit variants",
        ),
    ] {
        let error = expand_enum(&input).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn unsupported_serde_transforms_are_not_silently_ignored() {
    for input in [
        parse_quote! { #[serde(default)] struct A { x: String } },
        parse_quote! { #[serde(tag = "kind")] struct A { x: String } },
        parse_quote! { #[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))] struct A { x: String } },
        parse_quote! { struct A { #[serde(flatten)] x: String } },
        parse_quote! { struct A { #[serde(default)] x: String } },
        parse_quote! { struct A { #[serde(default = "make_default")] x: Option<String> } },
        parse_quote! { struct A { #[serde(alias = "y")] x: String } },
        parse_quote! { struct A { #[serde(rename(serialize = "x", deserialize = "y"))] x: String } },
        parse_quote! { struct A { #[serde(with = "adapter")] x: String } },
        parse_quote! { struct A { #[serde(serialize_with = "adapter")] x: String } },
        parse_quote! { struct A { #[serde(deserialize_with = "adapter")] x: String } },
        parse_quote! { struct A { #[serde(skip)] x: String } },
    ] {
        assert!(expand_model(&input, false).is_err());
    }
    for input in [
        parse_quote! { #[serde(tag = "kind")] enum A { X } },
        parse_quote! { #[serde(untagged)] enum A { X } },
        parse_quote! { enum A { #[serde(alias = "y")] X } },
        parse_quote! { enum A { #[serde(skip)] X } },
    ] {
        assert!(expand_enum(&input).is_err());
    }
}

#[test]
fn ambiguous_names_and_invalid_annotations_fail_locally() {
    for input in [
        parse_quote! { struct A { #[serde(rename = "same")] x: String, #[serde(rename = "same")] y: bool } },
        parse_quote! { #[serde(rename_all = "other")] struct A { x: String } },
        parse_quote! { struct A { #[openai(title = " ")] x: String } },
        parse_quote! { struct A { #[openai(min_length = -1)] x: String } },
        parse_quote! { struct A { #[openai(minimum = compute())] x: f64 } },
        parse_quote! { struct A { #[openai(multiple_of = compute())] x: f64 } },
        parse_quote! { struct A { #[openai(pattern = "a", pattern = "b")] x: String } },
        parse_quote! { struct A { #[openai(nullable)] x: String } },
        parse_quote! { struct A { #[openai(crate = "elsewhere")] x: String } },
        parse_quote! { #[openai(title = "Wrong location")] struct A { x: String } },
        parse_quote! { #[openai(crate = "not a path")] struct A { x: String } },
        parse_quote! { #[openai(patch_serialize)] struct A { x: String } },
    ] {
        assert!(expand_model(&input, false).is_err());
    }
    for input in [
        parse_quote! { #[openai(patch_serialize = true)] struct A { x: String } },
        parse_quote! { #[openai(patch_serialize, patch_serialize)] struct A { x: String } },
    ] {
        assert!(expand_model(&input, true).is_err());
    }
}

#[test]
fn serde_field_case_rules_preserve_wire_names() {
    use super::attrs::RenameRule;
    for (rule, expected) in [
        (RenameRule::Lower, "very_tasty"),
        (RenameRule::Upper, "VERY_TASTY"),
        (RenameRule::Pascal, "VeryTasty"),
        (RenameRule::Camel, "veryTasty"),
        (RenameRule::Snake, "very_tasty"),
        (RenameRule::ScreamingSnake, "VERY_TASTY"),
        (RenameRule::Kebab, "very-tasty"),
        (RenameRule::ScreamingKebab, "VERY-TASTY"),
    ] {
        assert_eq!(rule.field("very_tasty"), expected);
    }
}
