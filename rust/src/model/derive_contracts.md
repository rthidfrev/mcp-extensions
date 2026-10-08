The optional derives describe the same native fields as a handwritten `Model`.
They support owned flat generic structs, but reject Serde changes that would make
the described field names, requiredness or representation inaccurate.

An owned generic model adds only field description bounds. A complete settings
type only serializes; its generated patch only deserializes unless explicitly
opting into patch serialization:

```rust
use openai_mcp_extensions::{Model, SettingsModel};
use openai_mcp_extensions::forms::{FormResult, TypedForm, TypedFormResult};
use openai_mcp_extensions::settings::TypedSettings;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Model)]
struct Answer<T> {
    value: T,
    #[serde(default)]
    #[openai(pattern = "^[A-Z][a-z]+$")]
    note: Option<String>,
}
let form = TypedForm::<Answer<u8>>::new()?;
let TypedFormResult::Accept { content } = form.decode_result(FormResult::Accept {
    content: json!({"value":4}).as_object().unwrap().clone(),
})? else { panic!("accepted answer expected"); };
assert_eq!(content.value, 4);
assert_eq!(content.note, None);

#[derive(Serialize, SettingsModel)]
struct Preferences<T> {
    value: T,
    #[openai(multiple_of = 2)]
    count: u8,
}
let settings = TypedSettings::<Preferences<String>>::new()?;
let patch = settings.decode_patch(json!({"count":0}).as_object().unwrap().clone())?;
assert_eq!(patch.value, None);
assert_eq!(patch.count, Some(0));
# Ok::<(), openai_mcp_extensions::Error>(())
```

Flattening introduces fields outside the declared flat model:

```compile_fail
use openai_mcp_extensions::Model;
#[derive(Model)]
struct Invalid {
    #[serde(flatten)]
    fields: std::collections::HashMap<String, String>,
}
```

Different serialization and deserialization names do not describe one contract:

```compile_fail
use openai_mcp_extensions::Model;
#[derive(Model)]
struct Invalid {
    #[serde(rename(serialize = "sent", deserialize = "received"))]
    value: String,
}
```

Custom defaults can replace an omitted value, unlike bare `default` on `Option`:

```compile_fail
use openai_mcp_extensions::Model;
#[derive(Model)]
struct Invalid {
    #[serde(default = "custom_default")]
    note: Option<String>,
}
fn custom_default() -> Option<String> { Some("substitute".into()) }
```

Aliases accept more input names than the declared field contract:

```compile_fail
use openai_mcp_extensions::Model;
#[derive(Model)]
struct Invalid {
    #[serde(alias = "legacy")]
    value: String,
}
```

Borrowed lifetime parameters are outside the owned model contract:

```compile_fail
use openai_mcp_extensions::Model;
#[derive(Model)]
struct Invalid<'a> { value: &'a str }
```

Complete settings must contain concrete fields; omission belongs to the patch:

```compile_fail
use openai_mcp_extensions::SettingsModel;
use serde::Serialize;
#[derive(Serialize, SettingsModel)]
struct Invalid { value: Option<String> }
```
