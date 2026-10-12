# OpenAI MCP Extensions derives

Version `0.1.0-alpha.1` is experimental and unpublished; its API may change.

Optional procedural macros for the [Rust server SDK](../README.md#using-the-sdk).
Enable the SDK's `derive` feature and import its reexported `Model`, `SettingsModel`
and `StringEnum` derives. The helper generates ordinary native model descriptions;
it implements no transport, persistence, authorization or TypeScript generation.

The SDK remains usable without this package. The derives only implement the
native [form](../README.md#forms) and [settings](../README.md#settings) contracts.
Building a form or settings definition validates those fields through the same
runtime code as handwritten models. No requests, callbacks or persistence occur
during macro expansion or definition construction.

> **TODO(release):** After publishing both SDK packages, add their actual
> versioned API-reference URLs and verify the links to the SDK guide in the
> published README. The parent README is available in this repository but is not
> included in this macro package's archive.

## Models and constraints

`Model` describes a named struct with flat `FieldType` fields: native primitives,
string enums, string arrays (forms only), and one `Option<T>` wrapper. It adds no
Serde bound. A typed form additionally requires owned deserialization. A custom
`FieldType` implementation must accurately describe its type's wire values.
Numeric inputs follow the SDK's [typed form conversion rules](../README.md#forms):
integral JSON numbers decode within the Rust integer bounds, and floating-point
rounding must preserve the annotated constraints. The derives add no separate
conversion behavior.

```rust
use openai_mcp_extensions::Model;
use serde::Deserialize;

#[derive(Deserialize, Model)]
struct Answer<T> {
    value: T,
    #[serde(default)]
    #[openai(pattern = "^[A-Z][a-z]+$", min_length = 1, max_length = 20)]
    note: Option<String>,
}
```

Field annotations support `title`, `description`, `minimum`, `maximum`,
`min_length`, `max_length`, `pattern` and `multiple_of`. Numeric constraints must
be numeric literals; `pattern` must be a string literal. These map to the native
protocol keywords (for example, `multiple_of = 0.5` emits `multipleOf: 0.5`).
`Model::fields()` passes annotations to `ModelField::with_annotations`, which
rejects invalid patterns, incompatible constraints, nonpositive multiples and
widening of the field's Rust domain. A valid derive therefore does not guarantee
that every annotated definition is valid; callers must handle construction errors.

Serde field `rename`, container `rename_all`, `deny_unknown_fields`, and a
configured Serde `crate` path are supported. A bare field `serde(default)` is
accepted on `Option<T>` because omitted values already decode to `None`; it does
not insert a protocol default or accept JSON `null`. Required-field defaults,
custom defaults, aliases, flattening, skipped fields, adapters and distinct
serialization/deserialization names are rejected. Arbitrary Serde projections
need a handwritten model rather than inferred fields.

## Complete settings and partial updates

`SettingsModel` implements `Model` for complete values and generates a sibling
`<Name>Patch` with matching field visibility, JSON names, titles and constraints.
Every patch field is `Option<T>`: `None` means omitted, and `Some(false)`,
`Some(0)` and `Some("")` remain actual updates. Native settings reject empty,
unknown, null or invalid updates before invoking the update callback. Complete
values cannot contain optional fields, including aliases or generic instantiations
whose `FieldType::OPTIONAL` is true. Settings continue to reject array fields.

The generated patch implements `Model`, `Deserialize` and `Default`. Its default
contains only `None` and requires no `Default` implementation on field types.
The SDK serializes complete values and decodes patches, so complete values need
`Serialize`, patches need owned deserialization, and neither needs the opposite
operation. To serialize patches yourself, opt into the convenience implementation:

```rust
use openai_mcp_extensions::SettingsModel;
use serde::Serialize;

#[derive(Serialize, SettingsModel)]
#[openai(patch_serialize)]
struct Preferences<T> {
    value: T,
    #[openai(multiple_of = 2, minimum = 0, maximum = 100)]
    count: u8,
}
```

With `patch_serialize`, absent patch fields are omitted from JSON. Without it,
even a complete model with a manual whole-model serializer and a deserialization
only field can use the native settings API; an unused patch serializer introduces
no additional requirement.

## Generics, enums and limits

Owned generic named structs preserve their declared bounds and `where` clauses.
Generated `Model` implementations add `FieldType` bounds on actual field types;
patch Serde implementations add only the corresponding field serialization or
deserialization bounds. No blanket `Default`, `Serialize` or `Deserialize` bound
is added to every generic parameter. A generic instantiation must still have
supported flat field types and matching Serde behavior. Lifetime parameters and
borrowed models are unsupported; generic support is not arbitrary Serde schema
projection. Nullable fields, nested optional fields and nested objects remain
unsupported by native definition validation.

`StringEnum` describes nonempty unit enums without generic parameters. It uses
each variant's actual `Serialize` output, including Serde renames; duplicate or
nonstring values fail when building the schema. Decoding additionally needs
`Deserialize`. For a renamed SDK dependency, set
`#[openai(crate = "your_sdk_name")]` on the model or enum. Generated Serde paths
also use that SDK path, including for generic patches.

The runtime and macro packages have matching versions. Publish the macro package
before the runtime package. Until that first publication, local archive checks
must explicitly supply the unpublished dependency; see the SDK's
[development commands](../README.md#contributing). CI also rebuilds
the archived runtime using the archived macros, without publishing either package.
