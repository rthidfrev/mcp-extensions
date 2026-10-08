//! Optional derives for the SDK's supported flat Rust model contract.
//! Generated definitions use native SDK validation; macros perform no I/O.

mod attrs;

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{ToTokens, format_ident, quote};
use syn::{
    Data, DeriveInput, Fields, Generics, Ident, LitStr, Path, Type, ext::IdentExt,
    parse_macro_input, parse_quote,
};

use attrs::{FieldAttrs, root_attrs, serde_attrs};

/// Describe an owned named struct for typed forms and manual settings models.
///
/// Supports field `rename`, container `rename_all`, and flat `FieldType` values.
/// Field `openai` annotations add titles, descriptions and narrower constraints,
/// including `pattern` and `multiple_of`. A bare `serde(default)` on `Option<T>`
/// is accepted because omission already means `None`; custom defaults are rejected.
/// Generic implementations require each field type to implement `FieldType`,
/// preserve the original bounds, and add no serialization/deserialization bound.
/// Lifetime parameters are unsupported; forms require owned deserialization.
/// Unsupported Serde transformations produce a compilation error. Definition
/// validation remains fallible through the generated `Model::fields` method.
#[proc_macro_derive(Model, attributes(openai, serde))]
pub fn derive_model(input: TokenStream) -> TokenStream {
    finish(expand_model(
        &parse_macro_input!(input as DeriveInput),
        false,
    ))
}

/// Describe complete settings and generate a partial `<Name>Patch` struct.
///
/// Complete settings cannot contain optional fields. The patch has `Option<T>`
/// fields with matching JSON names and implements `Default`, `Deserialize`, and
/// `Model`. Add `#[openai(patch_serialize)]` to also derive `Serialize` for the patch;
/// that implementation omits absent fields. This derive
/// already implements `Model` for the complete struct; do not combine the two.
/// `Default` sets every field to `None` without requiring field defaults. Generic
/// patches implement Serde only when their fields support the respective operation.
/// The complete struct needs `Serialize`, and its patch needs `DeserializeOwned`;
/// complete values do not need to implement `Deserialize`.
#[proc_macro_derive(SettingsModel, attributes(openai, serde))]
pub fn derive_settings(input: TokenStream) -> TokenStream {
    finish(expand_model(
        &parse_macro_input!(input as DeriveInput),
        true,
    ))
}

/// Describe the actual serialized strings of a nonempty unit-variant enum.
///
/// Requires `Serialize`. `FieldType::schema` serializes each variant and returns
/// a local error for duplicate or nonstring values, including custom serializers.
/// Derive `Deserialize` as well when decoding the enum in forms or settings patches.
#[proc_macro_derive(StringEnum, attributes(openai, serde))]
pub fn derive_string_enum(input: TokenStream) -> TokenStream {
    finish(expand_enum(&parse_macro_input!(input as DeriveInput)))
}

fn finish(result: syn::Result<Tokens>) -> TokenStream {
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}

struct ModelField<'a> {
    source: &'a syn::Field,
    ident: &'a Ident,
    name: LitStr,
    title: LitStr,
    annotations: FieldAttrs,
}

fn owned(input: &DeriveInput) -> syn::Result<()> {
    if let Some(lifetime) = input.generics.lifetimes().next() {
        return Err(syn::Error::new_spanned(
            lifetime,
            "flat model derives support owned fields, not lifetime parameters; use a manual Model implementation",
        ));
    }
    Ok(())
}

fn named_fields(input: &DeriveInput, settings: bool) -> syn::Result<Vec<ModelField<'_>>> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "Model requires a named-field struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "Model requires named fields",
        ));
    };
    let root = serde_attrs(&input.attrs, true, false)?;
    let mut names = HashSet::new();
    fields.named.iter().map(|source| {
        let ident = source.ident.as_ref().expect("named field");
        let serde = serde_attrs(&source.attrs, false, optional(&source.ty))?;
        let annotations = FieldAttrs::parse(&source.attrs)?;
        let name = serde.rename.unwrap_or_else(|| {
            let name = ident.unraw().to_string();
            LitStr::new(&root.rename_all.map_or_else(|| name.clone(), |rule| rule.field(&name)), ident.span())
        });
        if !names.insert(name.value()) {
            return Err(syn::Error::new_spanned(&name, "duplicate effective JSON field name"));
        }
        if settings && optional(&source.ty) {
            return Err(syn::Error::new_spanned(
                &source.ty,
                "complete settings require every field; Option is only used in the generated patch",
            ));
        }
        let title = annotations.title.clone().unwrap_or_else(|| name.clone());
        if title.value().trim().is_empty() {
            return Err(syn::Error::new_spanned(&name, "an empty JSON name requires an explicit nonblank openai title"));
        }
        Ok(ModelField { source, ident, name, title, annotations })
    }).collect()
}

fn optional(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => {
            path.qself.is_none()
                && path
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "Option")
        }
        Type::Group(group) => optional(&group.elem),
        Type::Paren(paren) => optional(&paren.elem),
        _ => false,
    }
}

fn expand_model(input: &DeriveInput, settings: bool) -> syn::Result<Tokens> {
    owned(input)?;
    if settings {
        for attr in input.attrs.iter().filter(|a| a.path().is_ident("derive")) {
            let derives = attr.parse_args_with(
                syn::punctuated::Punctuated::<Path, syn::Token![,]>::parse_terminated,
            )?;
            if derives
                .iter()
                .any(|p| p.segments.last().is_some_and(|s| s.ident == "Model"))
            {
                return Err(syn::Error::new_spanned(
                    attr,
                    "SettingsModel already implements Model; derive SettingsModel without Model",
                ));
            }
        }
    }
    let root = root_attrs(&input.attrs, settings)?;
    let sdk = &root.sdk;
    let fields = named_fields(input, settings)?;
    let name = &input.ident;
    let model = model_impl(name, &input.generics, &fields, sdk, false, settings);
    if !settings {
        return Ok(model);
    }

    let patch = format_ident!("{}Patch", name.unraw());
    let visibility = &input.vis;
    let serde_path = LitStr::new(&quote!(#sdk::__private::serde).to_string(), name.span());
    // Per-field bounds describe the generated operations; blanket T bounds can
    // unnecessarily constrain generic wrappers or associated field types.
    let serialize_bounds = fields.iter().map(|field| {
        let ty = &field.source.ty;
        quote!(#ty: #sdk::__private::serde::Serialize)
    });
    let deserialize_bounds = fields.iter().map(|field| {
        let ty = &field.source.ty;
        quote!(#ty: #sdk::__private::serde::de::DeserializeOwned)
    });
    let serialize_bounds = LitStr::new(&quote!(#(#serialize_bounds),*).to_string(), name.span());
    let deserialize_bounds =
        LitStr::new(&quote!(#(#deserialize_bounds),*).to_string(), name.span());
    let serialize_derive = root
        .patch_serialize
        .then(|| quote!(#sdk::__private::serde::Serialize,));
    let serde_bounds = if root.patch_serialize {
        quote!(bound(serialize = #serialize_bounds, deserialize = #deserialize_bounds))
    } else {
        quote!(bound(deserialize = #deserialize_bounds))
    };
    let patch_fields = fields.iter().map(|field| {
        let ident = field.ident;
        let visibility = &field.source.vis;
        let ty = &field.source.ty;
        let name = &field.name;
        let docs = field
            .source
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("doc"));
        quote! {
            #(#docs)*
            #[serde(rename = #name, skip_serializing_if = "::std::option::Option::is_none")]
            #visibility #ident: ::std::option::Option<#ty>
        }
    });
    let patch_model = model_impl(&patch, &input.generics, &fields, sdk, true, false);
    let generics = &input.generics;
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let defaults = fields.iter().map(|field| {
        let ident = field.ident;
        quote!(#ident: ::std::option::Option::None)
    });
    let mut settings_generics = model_generics(generics, &fields, sdk, false);
    let predicates = &mut settings_generics.make_where_clause().predicates;
    predicates.push(parse_quote!(#name #type_generics: #sdk::__private::serde::Serialize));
    predicates
        .push(parse_quote!(#patch #type_generics: #sdk::__private::serde::de::DeserializeOwned));
    let (settings_impl_generics, _, settings_where_clause) = settings_generics.split_for_impl();
    let patch_doc = format!(
        "Partial replacement for `{name}`; `None` denotes an omitted update. \
         Serialization, when enabled with `patch_serialize`, omits absent fields."
    );
    Ok(quote! {
        #model
        #[doc = #patch_doc]
        #[derive(#serialize_derive #sdk::__private::serde::Deserialize)]
        #[serde(crate = #serde_path, deny_unknown_fields, #serde_bounds)]
        #visibility struct #patch #generics #where_clause { #(#patch_fields,)* }
        impl #impl_generics ::std::default::Default for #patch #type_generics #where_clause {
            fn default() -> Self {
                Self { #(#defaults,)* }
            }
        }
        #patch_model
        impl #settings_impl_generics #sdk::settings::SettingsModel for #name #type_generics #settings_where_clause {
            type Patch = #patch #type_generics;
        }
    })
}

fn model_impl(
    name: &Ident,
    generics: &Generics,
    fields: &[ModelField<'_>],
    sdk: &Path,
    patch: bool,
    settings: bool,
) -> Tokens {
    let generics = model_generics(generics, fields, sdk, patch);
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let definitions = fields.iter().map(|field| {
        let source_type = &field.source.ty;
        let ty = if patch {
            quote!(::std::option::Option<#source_type>)
        } else {
            source_type.to_token_stream()
        };
        let name = &field.name;
        let title = &field.title;
        let annotations = field.annotations.annotations(sdk);
        let complete = if settings {
            let path = format!("/{}", name.value().replace('~', "~0").replace('/', "~1"));
            quote! {
                if <#source_type as #sdk::model::FieldType>::OPTIONAL {
                    return ::std::result::Result::Err(#sdk::Error::ModelValidation {
                        path: #path.into(),
                        message: "complete settings cannot contain optional fields".into(),
                    });
                }
            }
        } else {
            Tokens::new()
        };
        let annotated = if annotations.is_empty() {
            quote!(field)
        } else {
            quote! {
                let mut annotations = #sdk::__private::serde_json::Map::new();
                #annotations
                field.with_annotations(annotations)?
            }
        };
        quote! {
            {
                #complete
                let field = #sdk::model::ModelField::new::<#ty>(#name, #title)?;
                #annotated
            }
        }
    });
    quote! {
        impl #impl_generics #sdk::model::Model for #name #type_generics #where_clause {
            fn fields() -> ::std::result::Result<::std::vec::Vec<#sdk::model::ModelField>, #sdk::Error> {
                ::std::result::Result::Ok(::std::vec![#(#definitions),*])
            }
        }
    }
}

fn model_generics(
    generics: &Generics,
    fields: &[ModelField<'_>],
    sdk: &Path,
    patch: bool,
) -> Generics {
    let mut generics = generics.clone();
    for field in fields {
        let ty = &field.source.ty;
        let ty = if patch {
            quote!(::std::option::Option<#ty>)
        } else {
            ty.to_token_stream()
        };
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#ty: #sdk::model::FieldType));
    }
    generics
}

fn expand_enum(input: &DeriveInput) -> syn::Result<Tokens> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "StringEnum requires a unit enum without generic parameters",
        ));
    }
    let sdk = root_attrs(&input.attrs, false)?.sdk;
    serde_attrs(&input.attrs, true, false)?;
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "StringEnum requires an enum",
        ));
    };
    if data.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "StringEnum requires at least one variant",
        ));
    }
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "StringEnum supports only unit variants",
            ));
        }
        serde_attrs(&variant.attrs, false, false)?;
        if let Some(attr) = variant.attrs.iter().find(|a| a.path().is_ident("openai")) {
            return Err(syn::Error::new_spanned(
                attr,
                "StringEnum has no per-variant openai annotations",
            ));
        }
    }
    let name = &input.ident;
    let variants = data.variants.iter().map(|v| &v.ident);
    Ok(quote! {
        impl #sdk::model::FieldType for #name {
            fn schema() -> ::std::result::Result<#sdk::__private::serde_json::Value, #sdk::Error> {
                let values = ::std::vec![#(#sdk::__private::serde_json::to_value(&Self::#variants)?),*];
                for (index, value) in values.iter().enumerate() {
                    if !value.is_string() || values[..index].contains(value) {
                        return ::std::result::Result::Err(#sdk::Error::ModelValidation {
                            path: ::std::string::String::new(),
                            message: "StringEnum variants must serialize as distinct strings".into(),
                        });
                    }
                }
                ::std::result::Result::Ok(#sdk::__private::serde_json::json!({"type":"string","enum":values}))
            }
        }
    })
}

#[cfg(test)]
mod tests;
