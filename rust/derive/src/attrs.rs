use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Expr, Lit, LitStr, Path, Token, UnOp, parse_quote};

#[derive(Default)]
pub(crate) struct SerdeAttrs {
    pub(crate) rename: Option<LitStr>,
    pub(crate) rename_all: Option<RenameRule>,
}

pub(crate) fn serde_attrs(
    attrs: &[Attribute],
    container: bool,
    optional_field: bool,
) -> syn::Result<SerdeAttrs> {
    let mut result = SerdeAttrs::default();
    let mut saw_crate = false;
    let mut saw_deny = false;
    let mut saw_default = false;
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let value: LitStr = meta.value()?.parse().map_err(|_| {
                    meta.error("use one string rename shared by serialization and deserialization")
                })?;
                set(&mut result.rename, value, &meta)?;
            } else if container && meta.path.is_ident("rename_all") {
                let value: LitStr = meta.value()?.parse().map_err(|_| {
                    meta.error("use one string rename_all shared by serialization and deserialization")
                })?;
                let rule = RenameRule::parse(&value)?;
                set(&mut result.rename_all, rule, &meta)?;
            } else if container && meta.path.is_ident("crate") {
                let value: LitStr = meta.value()?.parse()?;
                value.parse::<Path>()?;
                if saw_crate {
                    return Err(meta.error("duplicate serde crate attribute"));
                }
                saw_crate = true;
            } else if container && meta.path.is_ident("deny_unknown_fields") {
                if saw_deny || meta.input.peek(Token![=]) || meta.input.peek(syn::token::Paren) {
                    return Err(meta.error("expected one bare deny_unknown_fields attribute"));
                }
                saw_deny = true;
            } else if !container && meta.path.is_ident("default") {
                if !optional_field || meta.input.peek(Token![=]) || meta.input.peek(syn::token::Paren) {
                    return Err(meta.error(
                        "only bare serde(default) on an Option field is redundant; custom defaults and required-field defaults need a manual Model implementation",
                    ));
                }
                if saw_default {
                    return Err(meta.error("duplicate serde default attribute"));
                }
                saw_default = true;
            } else {
                return Err(meta.error(
                    "unsupported Serde transformation for a flat model; use a manual Model implementation",
                ));
            }
            Ok(())
        })?;
    }
    Ok(result)
}

fn set<T>(
    slot: &mut Option<T>,
    value: T,
    meta: &syn::meta::ParseNestedMeta<'_>,
) -> syn::Result<()> {
    if slot.replace(value).is_some() {
        Err(meta.error("duplicate attribute"))
    } else {
        Ok(())
    }
}

pub(crate) struct RootAttrs {
    pub(crate) sdk: Path,
    pub(crate) patch_serialize: bool,
}

pub(crate) fn root_attrs(attrs: &[Attribute], settings: bool) -> syn::Result<RootAttrs> {
    let mut result = None;
    let mut patch_serialize = false;
    for attr in attrs.iter().filter(|a| a.path().is_ident("openai")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("patch_serialize") && settings {
                if patch_serialize || meta.input.peek(Token![=]) || meta.input.peek(syn::token::Paren) {
                    return Err(meta.error("expected one bare openai(patch_serialize) attribute"));
                }
                patch_serialize = true;
                return Ok(());
            }
            if !meta.path.is_ident("crate") {
                return Err(meta.error(
                    "openai(crate = \"path\") is supported on types; bare patch_serialize is supported only on SettingsModel",
                ));
            }
            let value: LitStr = meta.value()?.parse()?;
            let mut path: Path = value.parse()?;
            if path.leading_colon.is_none()
                && !path
                    .segments
                    .first()
                    .is_some_and(|s| s.ident == "crate" || s.ident == "self" || s.ident == "super")
            {
                path.leading_colon = Some(Default::default());
            }
            set(&mut result, path, &meta)
        })?;
    }
    Ok(RootAttrs {
        sdk: result.unwrap_or_else(|| parse_quote!(::openai_mcp_extensions)),
        patch_serialize,
    })
}

#[derive(Default)]
pub(crate) struct FieldAttrs {
    pub(crate) title: Option<LitStr>,
    description: Option<LitStr>,
    minimum: Option<Expr>,
    maximum: Option<Expr>,
    multiple_of: Option<Expr>,
    pattern: Option<LitStr>,
    min_length: Option<syn::LitInt>,
    max_length: Option<syn::LitInt>,
}

impl FieldAttrs {
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut result = Self::default();
        for attr in attrs.iter().filter(|a| a.path().is_ident("openai")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("title") {
                    let value: LitStr = meta.value()?.parse()?;
                    if value.value().trim().is_empty() {
                        return Err(meta.error("field title must not be blank"));
                    }
                    set(&mut result.title, value, &meta)
                } else if meta.path.is_ident("description") {
                    let value = meta.value()?.parse()?;
                    set(&mut result.description, value, &meta)
                } else if meta.path.is_ident("pattern") {
                    let value = meta.value()?.parse()?;
                    set(&mut result.pattern, value, &meta)
                } else if meta.path.is_ident("minimum")
                    || meta.path.is_ident("maximum")
                    || meta.path.is_ident("multiple_of")
                {
                    let value: Expr = meta.value()?.parse()?;
                    if !numeric_literal(&value) {
                        return Err(meta.error("numeric constraints must be numeric literals"));
                    }
                    let slot = if meta.path.is_ident("minimum") {
                        &mut result.minimum
                    } else if meta.path.is_ident("maximum") {
                        &mut result.maximum
                    } else {
                        &mut result.multiple_of
                    };
                    set(slot, value, &meta)
                } else if meta.path.is_ident("min_length") || meta.path.is_ident("max_length") {
                    let value: syn::LitInt = meta.value()?.parse()?;
                    value.base10_parse::<u64>()?;
                    let slot = if meta.path.is_ident("min_length") {
                        &mut result.min_length
                    } else {
                        &mut result.max_length
                    };
                    set(slot, value, &meta)
                } else {
                    Err(meta.error("unsupported openai field attribute"))
                }
            })?;
        }
        Ok(result)
    }

    pub(crate) fn annotations(&self, sdk: &Path) -> TokenStream {
        let items = [
            self.description.as_ref().map(|value| quote! {
                annotations.insert("description".into(), #sdk::__private::serde_json::json!(#value));
            }),
            self.minimum.as_ref().map(|value| quote! {
                annotations.insert("minimum".into(), #sdk::__private::serde_json::json!(#value));
            }),
            self.maximum.as_ref().map(|value| quote! {
                annotations.insert("maximum".into(), #sdk::__private::serde_json::json!(#value));
            }),
            self.multiple_of.as_ref().map(|value| quote! {
                annotations.insert("multipleOf".into(), #sdk::__private::serde_json::json!(#value));
            }),
            self.pattern.as_ref().map(|value| quote! {
                annotations.insert("pattern".into(), #sdk::__private::serde_json::json!(#value));
            }),
            self.min_length.as_ref().map(|value| quote! {
                annotations.insert("minLength".into(), #sdk::__private::serde_json::json!(#value));
            }),
            self.max_length.as_ref().map(|value| quote! {
                annotations.insert("maxLength".into(), #sdk::__private::serde_json::json!(#value));
            }),
        ];
        let items = items.into_iter().flatten();
        quote! { #(#items)* }
    }
}

fn numeric_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(value) => matches!(value.lit, Lit::Int(_) | Lit::Float(_)),
        Expr::Unary(value) if matches!(value.op, UnOp::Neg(_)) => {
            matches!(value.expr.as_ref(), Expr::Lit(v) if matches!(v.lit, Lit::Int(_) | Lit::Float(_)))
        }
        _ => false,
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    fn parse(value: &LitStr) -> syn::Result<Self> {
        match value.value().as_str() {
            "lowercase" => Ok(Self::Lower),
            "UPPERCASE" => Ok(Self::Upper),
            "PascalCase" => Ok(Self::Pascal),
            "camelCase" => Ok(Self::Camel),
            "snake_case" => Ok(Self::Snake),
            "SCREAMING_SNAKE_CASE" => Ok(Self::ScreamingSnake),
            "kebab-case" => Ok(Self::Kebab),
            "SCREAMING-KEBAB-CASE" => Ok(Self::ScreamingKebab),
            _ => Err(syn::Error::new_spanned(
                value,
                "unsupported Serde rename_all rule",
            )),
        }
    }

    pub(crate) fn field(self, name: &str) -> String {
        match self {
            Self::Lower | Self::Snake => name.into(),
            Self::Upper | Self::ScreamingSnake => name.to_ascii_uppercase(),
            Self::Kebab => name.replace('_', "-"),
            Self::ScreamingKebab => name.to_ascii_uppercase().replace('_', "-"),
            Self::Pascal | Self::Camel => {
                let mut result = String::new();
                let mut word_start = true;
                for ch in name.chars() {
                    if ch == '_' {
                        word_start = true;
                    } else {
                        result.push(if word_start {
                            ch.to_ascii_uppercase()
                        } else {
                            ch
                        });
                        word_start = false;
                    }
                }
                if matches!(self, Self::Camel)
                    && let Some(first) = result.get_mut(..1)
                {
                    first.make_ascii_lowercase();
                }
                result
            }
        }
    }
}
