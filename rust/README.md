# OpenAI MCP Extensions for Rust

Add OpenAI's mentions, settings, forms, app entrypoints and display annotations to
an existing [rmcp](https://github.com/modelcontextprotocol/rust-sdk) server.
This crate implements the server side of the [extension specification](../docs/spec.md).
Your server keeps its transport, request controls and application state.

The core works directly with rmcp, including for HTML applications. Optional
Cargo features add model derives and checked MCP Apps conveniences. Browser
code runs in the host's application iframe and uses its MCP Apps bridge;
this crate provides the server APIs, not that browser runtime.

- [Installation](#installation)
- [Cargo features](#cargo-features)
- [Using the SDK](#using-the-sdk)
- [API reference](#api-reference)
- [Contributing](#contributing)

## Installation

Requires **Rust 1.88+** and **rmcp 3.5**. The crate is available from this checkout
and has not been published to crates.io. Version `0.1.0-alpha.1` is experimental;
its API may change while protocol and host compatibility are being validated.

Add a path dependency to your server's `Cargo.toml`:

```toml
[dependencies]
openai-mcp-extensions = { path = "../mcp-extensions/rust", default-features = false }
rmcp = { version = "3.5.0", features = ["server"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Enable your chosen transport on `rmcp`. The runnable examples use its
`transport-io` feature and Tokio.

### Cargo features

No optional features are enabled by default. Choose them in your server's
SDK dependency; they add APIs without changing the core's behavior:

| Feature    | Adds                                                                                                          | Additional dependency                        |
| ---------- | ------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| None       | Mentions, settings, forms, client context, file hints and OpenAI UI annotations on rmcp types                 | No optional SDK dependency                   |
| `derive`   | `Model`, `SettingsModel` and `StringEnum` derives for the native form/settings traits                         | `openai-mcp-extensions-macros`               |
| `mcp-apps` | Checked app-resource associations, HTML-content construction and advertised client-support inspection in `ui` | `mcp-apps-server`, with its `server` feature |

For example, enable only the model derives:

```toml
openai-mcp-extensions = { path = "../mcp-extensions/rust", default-features = false, features = ["derive"] }
```

Use `features = ["mcp-apps"]` for the app helpers, or
`features = ["derive", "mcp-apps"]` for both. The derives implement native
traits; they do not register tools or send requests. The app helpers prepare
metadata and contents; enabling them does not install a router or launch a UI.

With `mcp-apps` disabled, the SDK does not compile MCP Apps Server and its helper
APIs and error variants are absent. OpenAI entrypoints and display annotations
remain available, so you can serve apps directly with rmcp or select another
generic integration. To use `AppRouter`, `AppServer` or typed security declarations,
declare MCP Apps Server directly in your application as described in
[HTML applications](#html-applications); the SDK does not reexport that library.

The optional integration uses the published
[`mcp-apps-server` alpha](https://docs.rs/crate/mcp-apps-server/0.1.0-alpha.1),
pinned to **`0.1.0-alpha.1`**. Its author has manually tested it only on
**ChatGPT Desktop**. Compatibility with other hosts and exhaustive feature
coverage have not been established. The API may change between alpha releases;
the exact version pin keeps dependency updates explicit. This qualification
concerns the optional MCP Apps library, not the SDK's core OpenAI APIs.

Cargo combines the features requested by all consumers of a package. If another
dependency enables `mcp-apps`, omitting it in one declaration does not disable it
for the application. Inspect the active graph with `cargo tree -e features`.
See Cargo's [feature rules](https://doc.rust-lang.org/cargo/reference/features.html#feature-unification)
for details.

## Using the SDK

### Connect it to your server

Your server keeps its rmcp handler, transport and application state. The SDK
provides tool routes, validated definitions and metadata helpers.

With a **ToolRouter**, create an `OpenAIExtensions` alongside your router,
register the features you need, then advertise their capabilities in
`ServerHandler::get_info`:

```rust
use openai_mcp_extensions::OpenAIExtensions;
use rmcp::{handler::server::router::tool::ToolRouter, model::ServerConfig};

let mut router: ToolRouter<()> = ToolRouter::new();
let mut extensions = OpenAIExtensions::new();
// Register mentions or settings here, as shown below.

let mut info = ServerConfig::default();
extensions.apply_capabilities(&mut info.capabilities);
// Return info from get_info; route list_tools/call_tool through this router.
```

Register extensions before serving. Keep the configured router and capability
advertisement together. Your callbacks handle authorization and application
effects; the SDK validates extension input and output.

Registration preserves existing tools. A name conflict returns an error, including
for disabled tools. Settings install their read/update pair together; failed
registration does not advertise those capabilities.

Start with the [handwritten server](examples/typed.rs), which exposes settings
and a quantity form without optional SDK features. From the repository root:

```sh
cargo run --manifest-path rust/Cargo.toml --example typed --no-default-features
```

Configure an MCP host to launch one of these commands. The process waits for
JSON-RPC on stdin; stdout is reserved for the protocol and diagnostics go to
stderr.

The [derived server](examples/derived.rs) exposes the same settings and form using
model derives. Run it with `cargo run --manifest-path rust/Cargo.toml --example derived --features derive`.
For HTML applications, follow the [direct rmcp and optional helper examples](#html-applications).

<details>
<summary>Integrate with a custom tool dispatcher instead of ToolRouter</summary>

Obtain routes without installing them:

| Feature        | Builder                                                       |
| -------------- | ------------------------------------------------------------- |
| Mentions       | `mentions::search_tool` or `search_tool_with_state`           |
| Raw settings   | `SettingsRegistration::into_tools` or `into_tools_with_state` |
| Typed settings | `TypedSettings::into_tools` or `into_tools_with_state`        |

Expose each route's `attr` through `tools/list`. After your common request
checks, call the selected route with the original request context:

```rust
use rmcp::{
    ErrorData, RoleServer,
    handler::server::{router::tool::ToolRoute, tool::ToolCallContext},
    model::{CallToolRequestParams, CallToolResponse},
    service::RequestContext,
};

async fn execute<S>(
    route: &ToolRoute<S>,
    server: &S,
    params: CallToolRequestParams,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResponse, ErrorData> {
    (route.call)(ToolCallContext::new(server, params, context)).await
}
```

Check both settings names, including disabled or reserved names, before
installing either route. Only then apply the returned `SettingsCapability` to
your server capabilities. Your registry owns atomic installation and subsequent
capability consistency. After installing the standalone mention route, call
`mentions::apply_capabilities` as well. The ToolRouter helpers perform that
preflight and capability tracking for you.

See the [custom server](examples/custom_server.rs) for complete dispatch,
request checks and form handling. Both integration modes use the same SDK
validation and execution.

</details>

### Find an extension

| I want to…                                                | Guide                                       |
| --------------------------------------------------------- | ------------------------------------------- |
| Offer resources in the composer's `@` picker              | [Mentions](#mentions)                       |
| Expose editable server preferences                        | [Settings](#settings)                       |
| Ask the user for structured input                         | [Forms](#forms)                             |
| Serve an HTML interface and choose its display modes      | [HTML applications](#html-applications)     |
| Launch that interface globally, in a thread or for a file | [Entrypoints](#entrypoints)                 |
| Read host file hints or choose text/blob resource content | [Resources and files](#resources-and-files) |
| Read session, user, locale or location metadata           | [Client context](#client-context)           |
| Inspect a failure's location and cause                    | [Errors](#errors)                           |

### Mentions

Register `search_mentions` so the host can search resources for its `@` picker.
The callback receives the query and request context, and returns matching items.
An empty query is valid. After successful registration, `apply_capabilities`
advertises `openai/mentions` with `searchTool: "search_mentions"` in both the
modern `extensions` and legacy `experimental` maps. The tool retains its legacy
mention metadata for hosts using tool-based discovery.

```rust
use openai_mcp_extensions::{
    OpenAIExtensions,
    mentions::{MentionItem, MentionSearchResult},
};
use rmcp::{handler::server::router::tool::ToolRouter, model::Resource};

let mut router: ToolRouter<()> = ToolRouter::new();
let mut extensions = OpenAIExtensions::new();
extensions.register_mentions(&mut router, |params, _context| async move {
    let resource = Resource::new("catalog://parts/bolt", "Hex bolt");
    let items = if resource.name.contains(&params.query) {
        vec![MentionItem::ResourceLink(resource)]
    } else {
        vec![]
    };
    Ok(MentionSearchResult { items })
})?;
```

Use `register_mentions_with_state` when the search borrows your server state.
Return `Box::pin(async move { ... })` from that callback. For a custom dispatcher,
use the corresponding standalone builder from the overview.

**API sources:** [mentions](src/mentions.rs) ·
[registration methods](src/extensions.rs).

### Settings

Settings expose two tools: `settings.read` returns the schema and all effective
values; `settings.update` receives a nonempty patch and returns all updated
values. The SDK validates the patch before invoking your callback. Your server
owns merging and persistence.

**Define settings from a Rust structure, with the `derive` feature:**

```rust
use openai_mcp_extensions::{SettingsModel, settings::TypedSettings};
use serde::Serialize;

#[derive(Clone, Serialize, SettingsModel)]
#[serde(rename_all = "camelCase")]
struct Preferences {
    #[openai(title = "Show grid")]
    show_grid: bool,
}

let settings = TypedSettings::<Preferences>::new()?;
```

The derive also creates `PreferencesPatch`, whose `show_grid: Option<bool>`
means `None` leaves the value unchanged and `Some(false)` updates it to false.
Complete values require `Serialize`; patches require owned `Deserialize`.

<details>
<summary>Define the same typed settings without the SDK derives</summary>

Replace the definitions above with handwritten traits. They use the same runtime:

```rust
use openai_mcp_extensions::{
    Error, Model, ModelField,
    settings::{SettingsModel, TypedSettings},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Preferences {
    show_grid: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreferencesPatch {
    show_grid: Option<bool>,
}

impl Model for Preferences {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![ModelField::new::<bool>("showGrid", "Show grid")?])
    }
}

impl Model for PreferencesPatch {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![ModelField::new::<Option<bool>>("showGrid", "Show grid")?])
    }
}

impl SettingsModel for Preferences {
    type Patch = PreferencesPatch;
}

let settings = TypedSettings::<Preferences>::new()?;
```

The two descriptions must have the same names and constraints; only patch
requiredness differs.

</details>

**Connect either definition to your state.** This example uses a Tokio mutex
and stores preferences in memory; replace its update body with your authorized,
atomic persistence operation. For this example, add
`tokio = { version = "1", features = ["sync"] }` to your dependencies:

```rust
use openai_mcp_extensions::OpenAIExtensions;
use rmcp::handler::server::router::tool::ToolRouter;
use tokio::sync::Mutex;

struct Server {
    preferences: Mutex<Preferences>,
}

let mut router: ToolRouter<Server> = ToolRouter::new();
let mut extensions = OpenAIExtensions::new();
extensions.register_typed_settings_with_state(
    &mut router,
    settings,
    |server: &Server, _context| {
        Box::pin(async move { Ok(server.preferences.lock().await.clone()) })
    },
    |server: &Server, patch: PreferencesPatch, _context| {
        Box::pin(async move {
            let mut values = server.preferences.lock().await;
            if let Some(show_grid) = patch.show_grid {
                values.show_grid = show_grid;
            }
            Ok(values.clone())
        })
    },
)?;
```

Values are concrete primitives: booleans, strings, numbers or integers. String
enums and primitive constraints are supported. Null, unknown fields and empty
patches are rejected; omission keeps the old value. The SDK does not insert
defaults or undo application writes if your callback returns invalid values.

<details>
<summary>Use a JSON schema directly, or customize names and layout</summary>

For dynamic definitions, use `SettingsRegistration` and
`register_settings` / `register_settings_with_state`. Read and update
callbacks return a `JsonObject`; update receives the validated replacement map.

```rust
use openai_mcp_extensions::settings::{
    SettingsGroup, SettingsLayoutItem, SettingsRegistration,
};
use serde_json::json;

let settings = SettingsRegistration::new(json!({
    "type": "object",
    "properties": {
        "showGrid": {"type": "boolean", "title": "Show grid"}
    },
    "required": ["showGrid"]
}))?
.with_tool_names("preferences.read", "preferences.update")?
.with_layout(vec![SettingsGroup::new(
    "Display",
    [SettingsLayoutItem::property("showGrid")],
)])?;
```

`TypedSettings` also supports `with_tool_names` and `with_layout`.
Typed patches use the [same numeric conversion rules as typed forms](#forms).
Layout groups can contain property controls or tool buttons. The server supplies
and authorizes those button tools. Settings schemas reject arrays, nested objects
and schema defaults.

</details>

**API sources:** [TypedSettings](src/settings/typed.rs) ·
[SettingsRegistration](src/settings.rs) ·
[SettingsModel derive guide](derive/README.md#complete-settings-and-partial-updates).

Tool layout entries need only the tool name. Their optional legacy `title` is
deprecated and ignored by hosts; set the referenced MCP tool's `title` instead.
`SettingsLayoutItem::tool("view_parts")` creates an entry without that legacy field.

### Forms

Build a form definition, then use the request flow supported by the client's MCP
version. An accepted answer is validated and deserialized into your Rust type;
cancellation and refusal remain distinct results.

**Define an answer with the `derive` feature:**

```rust
use openai_mcp_extensions::{Model, forms::TypedForm};
use serde::Deserialize;

#[derive(Deserialize, Model)]
struct Answer {
    #[openai(title = "Quantity", minimum = 1, maximum = 100)]
    quantity: u8,
    #[openai(title = "Optional note")]
    note: Option<String>,
}

let form = TypedForm::<Answer>::new()?;
```

`quantity` is required. Omitting `note` produces `None`; explicit JSON
`null` is invalid. Construction checks the definition but sends no request.
See the [derive guide](derive/README.md) for supported annotations and Serde naming.

Typed forms and settings first validate the submitted JSON, then prepare native
numbers for Serde. Integral values such as `1`, `1.0` and `1e0` decode to the same
Rust integer within its bounds; exact JSON integers are preserved, including
64-bit values above `2^53`. Floating-point values (`f32` and `f64`) are rounded as
Serde would round them, then checked against the field's schema again. If rounding crosses a bound or violates
another constraint, decoding fails at that field before any settings callback.
Use `f64` when the field needs greater precision. Strings and nulls are never
converted into numbers, and missing fields receive no implicit defaults.

These rules apply to handwritten and derived models. Custom `FieldType` values
are unchanged by default; custom Serde transformations must preserve the domain
declared by the consumer's model.

<details>
<summary>Define the same typed form without the SDK derives</summary>

```rust
use openai_mcp_extensions::{Error, Model, ModelField, forms::TypedForm};
use serde::Deserialize;

#[derive(Deserialize)]
struct Answer {
    quantity: u8,
    note: Option<String>,
}

impl Model for Answer {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<u8>("quantity", "Quantity")?.with_annotations(
                serde_json::Map::from_iter([
                    ("minimum".into(), 1.into()),
                    ("maximum".into(), 100.into()),
                ]),
            )?,
            ModelField::new::<Option<String>>("note", "Optional note")?,
        ])
    }
}

let form = TypedForm::<Answer>::new()?;
```

Names and schemas must describe the type's actual Serde representation.

</details>

**Choose the transport flow:**

| Client request                  | What the server does                                                                                                                           |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| Legacy MCP, before `2026-07-28` | Sends `openai/elicitation/create` and awaits accept, cancel or decline.                                                                        |
| MCP `2026-07-28` or later       | Returns an input-required result; the host displays the form and retries the tool with its response. This is MRTR (multi-round tool requests). |

For **legacy input**, call `form.elicit_input(&context, message).await`
inside your tool handler. The client must advertise `openai/elicitation` form
support. Handle each result explicitly:

```rust
use openai_mcp_extensions::forms::{TypedForm, TypedFormResult};
use rmcp::{ErrorData, RoleServer, model::CallToolResult, service::RequestContext};

async fn ask(
    form: &TypedForm<Answer>,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    match form.elicit_input(context, "Choose a quantity").await? {
        TypedFormResult::Accept { content } => Ok(CallToolResult::success(vec![
            rmcp::model::ContentBlock::text(format!("Quantity: {}", content.quantity)),
        ])),
        TypedFormResult::Cancel => Ok(CallToolResult::success(vec![
            rmcp::model::ContentBlock::text("Cancelled"),
        ])),
        TypedFormResult::Decline => Ok(CallToolResult::success(vec![
            rmcp::model::ContentBlock::text("Declined"),
        ])),
    }
}
```

For **modern input**, use `mrtr::is_mrtr_request` to select the modern handler,
then `mrtr::handle_tool_call` inside your server's request pipeline. Each modern
request must advertise both `elicitation.form: {}` and
`extensions["openai/elicitation"].form: {}`. These gates run before the callback.
The SDK embeds `elicitation/create` in `inputRequests`, with an empty core object
schema and the rich schema in `params._meta["openai/elicitation"].requestedSchema`.
In its callback:

1. Read `form.read_input(&params, "answer")`.
2. On `Initial`, save continuation state and return `FormInputRequired` built
   from `form.raw_form()`.
3. On `Response`, validate the saved state, then handle accept, cancel or decline.
   Return `FormToolResponse::Complete` with the final tool result.

The [shared typed server](examples/support/typed_server.rs) implements both flows.
The [custom server](examples/custom_server.rs) shows where to keep your common
checks and diagnostics around modern handling. `FormService` is also available
as an adapter when that interception suits your server.

For legacy waits, `elicit_input_with_options` forwards rmcp request options,
including timeouts. Cancelling the parent tool call does not automatically stop
that wait. The server owns cancellation handling and must check the request's
cancellation token before performing application effects. A form's `cancel`
result is a user response, distinct from cancellation of the MCP tool request.

> [!IMPORTANT]
> Continuation state is application-owned. Bind it to the authorized caller,
> original arguments and form, check expiry, and consume it atomically with any
> write. The examples' static markers are only for public, read-only demonstrations.

<details>
<summary>Dynamic schemas, rich choices, resources and runtime annotations</summary>

For dynamic forms, construct `OpenAIForm` with validated `FormField` values or
`OpenAIForm::from_json`. Use the raw `elicit_input` / `mrtr::read_input`
functions with it:

```rust
use openai_mcp_extensions::forms::{FormField, OpenAIForm};

let form = OpenAIForm::new(
    [("quantity".to_owned(), FormField::integer("Quantity"))],
    vec!["quantity".to_owned()],
)?;
```

Raw forms retain the OpenAI schema, including suggestions, titled choices,
descriptions, thumbnails, previews and resource selection. Useful builders include
`string_choices`, `string_suggestions`, `single_resource` and
`multiple_resources`. Resource inputs use ordinary rmcp `Resource` options;
preview targets use `PreviewTarget`.

With typed forms, `with_field_annotations` adds runtime options or narrows
constraints while retaining the Rust domain. It cannot change requiredness or
widen allowed values. Typed answers reject unknown properties. Raw forms allow
undeclared properties whose values are supported primitives or string arrays.
Defaults never supply missing required answers.

</details>

**API sources:** [TypedForm](src/forms/typed.rs) ·
[raw forms](src/forms.rs) ·
[modern handling](src/forms/mrtr.rs) ·
[Model derive guide](derive/README.md#models-and-constraints).

### HTML applications

An MCP App is an HTML resource associated with a tool through
`_meta.ui.resourceUri`. The server lists it and returns its HTML through
`resources/read` using `text/html;profile=mcp-app`. The host loads that HTML
into an isolated iframe. It may be a static file, a template or a bundled
interactive frontend.

Choose the server integration according to what you need:

| Integration                           | What your server supplies                                                                                    | Runnable example                                        |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------- |
| Direct rmcp; no optional SDK features | Tool association, resource descriptor/contents, capability declaration and resource dispatch                 | [rmcp_app](examples/rmcp_app.rs)                        |
| MCP Apps Server                       | Typed resource/security declarations and checked registration; your handler or its ready adapter serves them | [mcp_app](examples/mcp_app.rs), enabled with `mcp-apps` |

Both examples apply the same OpenAI entrypoints and resource display annotations,
serve the same HTML and return a typed tool result with ordinary text for clients
that cannot render apps. From the repository root:

```sh
cargo run --manifest-path rust/Cargo.toml --example rmcp_app --no-default-features
cargo run --manifest-path rust/Cargo.toml --example mcp_app --features mcp-apps
```

Each process waits for an MCP host on stdin/stdout. These are static server
registration examples; they do not initialize a browser bridge. Replace the
HTML with your application's complete bundle for interactive behavior.

#### Serve directly with rmcp

OpenAI display annotations work on native resource contents without `mcp-apps`:

```rust
use openai_mcp_extensions::{
    OpenAIUiResourceExt,
    ui::{DisplayMode, UiResourceMetadata},
};
use rmcp::model::ResourceContents;

let mut contents = ResourceContents::text(
    "<!doctype html><main>Parts</main>", "ui://parts/viewer",
).with_mime_type("text/html;profile=mcp-app");
contents.apply_openai_ui(&UiResourceMetadata {
    available_display_modes: Some(vec![DisplayMode::Inline, DisplayMode::Fullscreen]),
    preferred_display_mode: Some(DisplayMode::Inline),
})?;
```

Return those contents when `resources/read` requests their URI. Your handler
also returns a matching descriptor from `resources/list`, advertises
`io.modelcontextprotocol/ui`, and associates the tool with that URI before
registration. The [direct example](examples/rmcp_app.rs) implements all these
operations and prepares fixed generic metadata from private Rust structures.
The [entrypoint example](#entrypoints) shows the tool's OpenAI annotations.

The SDK validates its `openai/ui` profile and preserves other metadata. With
this direct path, your code owns validation of generic resource identities,
security declarations and conflicting associations. The fixed declarations in
the example are not a general resource registry.

#### Add checked MCP Apps conveniences

Enable `mcp-apps` for `ui::associate_app_resource`,
`Tool::associate_app_resource`, `ui::app_html_resource`,
`ui::supports_mcp_apps`, `APP_MIME_TYPE` and `APP_CAPABILITY_KEY`. These
helpers delegate to MCP Apps Server rather than implementing another generic
metadata format. They do not register or serve resources.

Association preserves existing visibility and unknown fields. A conflicting
`ui.resourceUri` or legacy `ui/resourceUri` returns `Error::Configuration`
without changing the tool; an identical association is accepted. Invalid URIs
and non-object `ui` metadata also fail without mutation. The source chain retains
the original MCP Apps error. The string helpers reject the empty `ui://`
identifier; the dependency's typed `UiResourceUri` uses its generic URI contract.
`supports_mcp_apps` returns false for absent or malformed client declarations;
use the dependency's `ui_capability` to retain that distinction and its error.

To use typed resource/security declarations or a router directly, add the
same published alpha to **your server's** manifest:

```toml
mcp-apps-server = { version = "=0.1.0-alpha.1", default-features = false, features = ["server"] }
```

This direct dependency can also be composed with the SDK's core without enabling
its `mcp-apps` feature. The feature is necessary only for the SDK's helper APIs.
The following prepares typed declarations without adopting a router:

```rust
use mcp_apps_server::{ResourceUi, ToolUi, UiCsp, UiResource, UiResourceUri};
use openai_mcp_extensions::{
    OpenAIUiResourceExt,
    ui::{DisplayMode, UiResourceMetadata},
};
use rmcp::model::{MetaObject, Tool};

let resource = UiResource::new(UiResourceUri::new("ui://parts/viewer")?, "parts-viewer");
let schema = serde_json::Map::from_iter([("type".into(), "object".into())]);
let mut tool = Tool::new("open_view", "Open the part viewer", schema);
ToolUi::new(resource.uri().clone()).apply_to(&mut tool)?;

let mut csp = UiCsp::default();
csp.connect_domains = Some(vec![]);
csp.resource_domains = Some(vec![]);
let mut ui = ResourceUi::default();
ui.csp = Some(csp);
let mut meta = MetaObject::default();
ui.merge_into(&mut meta)?;
let mut contents = resource.html_contents("<!doctype html><main>Parts</main>").with_meta(meta);
contents.apply_openai_ui(&UiResourceMetadata {
    available_display_modes: Some(vec![DisplayMode::Inline, DisplayMode::Fullscreen]),
    preferred_display_mode: Some(DisplayMode::Inline),
})?;
```

`resource.descriptor()` supplies the resource list entry. Return `contents`
from its read handler. `ToolUi` and `ResourceUi` preserve unrelated namespaces
and reject conflicting supplied values without a partial update.

<details>
<summary>Register resources while retaining your existing handler and forms</summary>

Register mentions/settings on your `ToolRouter` first, then pass it to
`AppRouter::from_tool_router`. Register the UI resources and call `validate()`
before serving. Advertise both `OpenAIExtensions::apply_capabilities` and
`AppRouter::declare_capabilities` in your handler.

Delegate requests with their original state and request context. Use
`contains_resource` to select UI resource requests before calling
`read_resource`; handle other resource namespaces in your handler. Callback
errors are actual errors, not signals to try another owner. The router supplies
unpaginated lists; your handler owns filtering or combined pagination.

The [complete server](examples/server.rs), which requires `mcp-apps`, combines
this router with settings, mentions, file hints and both form lifecycles. Its
handler and `FormService` retain the form request pipeline. `AppServer`, used
in the smaller [app example](examples/mcp_app.rs), supplies a ready tool/resource
handler; it does not wrap an existing handler.

</details>

<details>
<summary>Use MCP Apps Server's authoring macros</summary>

Enable `macros` on your direct MCP Apps Server dependency:

```toml
mcp-apps-server = { version = "=0.1.0-alpha.1", default-features = false, features = ["server", "macros"] }
```

`#[derive(mcp_apps_server::StaticUiResource)]` embeds an HTML file and prepares
its declaration. `#[mcp_apps_server::app_router]` associates annotated rmcp
tools with views. These macros belong to that library; neither SDK feature
`derive` nor `mcp-apps` enables them.

For OpenAI resource annotations, supply `meta = resource_meta` on the resource
derive. That function returns `Result<MetaObject, MetadataError>`; call
`UiResourceMetadata::apply` there and map its error with
`MetadataError::Preparation(Box::new(error))`. Apply tool entrypoints before
registration using `UiToolMetadata`. The dependency's
[API reference](https://docs.rs/mcp-apps-server/0.1.0-alpha.1/mcp_apps_server/)
describes the native declarations and macro attributes.

</details>

#### Browser behavior and host support

Browser code initializes its MCP Apps bridge and sends UI requests, including
model-context updates and messages. Choose that frontend independently of the
Rust server. Server tools still perform authorization when the bridge calls them.

The host enforces iframe security and chooses the display mode. Resource CSP and
permissions are declarations; they do not grant access or authorize tool calls.
With MCP Apps Server, use `ResourceUi`, `UiCsp` and `UiPermissions` for these
declarations, and `ToolUi` for visibility. On the direct path, your server must
supply the equivalent metadata. Advertised support does not establish actual
rendering, entrypoint support or permissions in a particular host.

**API sources:** [OpenAI UI annotations](src/ui.rs) ·
[optional HTML helper](src/ui/mcp_apps.rs) ·
[MCP Apps Server](https://docs.rs/mcp-apps-server/0.1.0-alpha.1/mcp_apps_server/).

### Entrypoints

Entrypoints tell the host where a user can launch an associated HTML application.
These OpenAI annotations are available without optional features. Apply them to
the existing tool declaration before registration:

```rust
use openai_mcp_extensions::{
    OpenAIUiToolExt,
    ui::{Entrypoint, UiToolMetadata},
};
use rmcp::model::{MetaObject, Tool};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppAssociation { resource_uri: &'static str }

let schema = serde_json::Map::from_iter([("type".into(), "object".into())]);
let mut tool = Tool::new("open_view", "Open the part viewer", schema);
// This tool is new, so its generic association cannot overwrite an existing one.
let mut meta = MetaObject::default();
meta.0.insert("ui".into(), serde_json::to_value(AppAssociation {
    resource_uri: "ui://parts/viewer",
})?);
tool.meta = Some(meta);
tool.apply_openai_ui(&UiToolMetadata {
    entrypoints: Some(vec![
        Entrypoint::Global { quick_action: None },
        Entrypoint::Thread,
        Entrypoint::File { extensions: vec![".stl".into()] },
    ]),
    ..Default::default()
})?;
```

| Entrypoint | Launch arguments                                  |
| ---------- | ------------------------------------------------- |
| `Global`   | `{}` from the global/sidebar launch               |
| `Thread`   | `{}` from a conversation launch                   |
| `Settings` | `{}` from the app's settings launch               |
| `File`     | `{"file": {"name": "...", "resourceUri": "..."}}` |

With `mcp-apps`, replace the manual association above with
`tool.associate_app_resource("ui://parts/viewer")?` to obtain URI/conflict checks.
In either path, applying entrypoints does not register a tool or a resource.

Global entrypoints can include a `QuickAction`; settings entrypoints can include
search terms. Titles and icons use native rmcp tool/server fields. These are host
declarations, so actual launch behavior depends on the host.

**API source:** [entrypoints and tool annotations](src/ui.rs).

### Resources and files

Ordinary MCP resources remain rmcp's responsibility. These helpers read the
OpenAI hints attached to a request:

| Request                        | Helper                                       | Result                               |
| ------------------------------ | -------------------------------------------- | ------------------------------------ |
| File-entrypoint tool arguments | `FileEntrypointInput`, then `validate()`     | Display name and resource URI        |
| Tool-call metadata             | `resource_tool_call_metadata(&context.meta)` | Optional host path                   |
| `resources/read` metadata      | `requested_representation(&context.meta)`    | Optional `Text` or `Blob` preference |

For server-owned resources, return rmcp text contents or base64 blob contents
according to the requested representation. The [complete server](examples/server.rs)
demonstrates these core helpers alongside optional MCP Apps routing; run it with
`--features mcp-apps`.

A resource URI or host path is not permission to access a local file. The server
may run on a different machine; file-entrypoint reads and browser file operations
remain host/application responsibilities.

**API source:** [resource inputs and metadata](src/resources.rs).

### Client context

Read client-provided metadata through named accessors on an rmcp request context:

```rust
use openai_mcp_extensions::{Error, OpenAIRequestContextExt};
use rmcp::{RoleServer, service::RequestContext};

fn conversation_id(context: &RequestContext<RoleServer>) -> Result<Option<&str>, Error> {
    context.openai_client_fields().session_id()
}
```

| Accessor                          | Meaning                                                  |
| --------------------------------- | -------------------------------------------------------- |
| `session_id()`                    | Anonymized conversation identifier from `openai/session` |
| `user_id()` / `organization_id()` | Client-provided user/organization identifiers            |
| `locale()`                        | Requested language                                       |
| `user_agent()` / `location()`     | Optional client hints                                    |

Each accessor validates only the field requested and returns `Result<Option<_>, Error>`.
A malformed location does not prevent reading a valid session. The same accessors
work on `ToolCallContext`; `ClientMetadataReader::new` accepts metadata directly.

An absent identifier stays absent. A conversation identifier differs from the
JSON-RPC request ID or MCP transport session. User-agent/location hints are not
authorization data. Use `openai_client()` when you deliberately want all known
fields to pass strict validation together.

**API source:** [client metadata and context accessors](src/client.rs).

### Errors

SDK errors expose a structured location with `Error::path()` and an underlying
cause with `std::error::Error::source()` when available. The location is a JSON
Pointer; an empty string means the whole value.

`McpAppsUri` and `McpAppsMetadata` are present only with `mcp-apps`. The core
error variants and their classification remain the same in both configurations.

`Error` is non-exhaustive: match the variants your application handles and keep
an `_` arm for other failures. This prevents another dependency enabling an SDK
feature from making your error match incomplete.

At the MCP boundary, invalid caller input produces invalid-parameters errors;
invalid definitions, callback output or legacy peer responses produce internal
errors. Local transport failures are distinct from input errors; peer MCP errors
retain their supplied code and data.

**API source:** [Error](src/error.rs).

## API reference

The README provides task-oriented examples. The source links in each guide lead
to the API documentation comments available in this repository. Rustdoc turns
those comments into a browsable reference for types, methods, accepted
definitions and errors. The SDK packages are not yet published, so their hosted
API reference is not available.

> **TODO(release):** After publishing `openai-mcp-extensions` and
> `openai-mcp-extensions-macros`, add their actual versioned API-reference URLs
> here and replace the API source links with matching hosted item links. Configure
> the hosted Rustdoc build to include the optional APIs and verify their pages.

From the repository root, generate the core reference without optional APIs:

```sh
cargo doc --manifest-path rust/Cargo.toml -p openai-mcp-extensions --no-default-features --no-deps --open
```

Cargo opens the generated SDK reference in your browser. To include the SDK's
optional helpers and derives too, run:

```sh
cargo doc --manifest-path rust/Cargo.toml --workspace --all-features --no-deps --open
```

This generates both SDK packages without documenting their dependencies.
The core reference omits optional helper/derive pages. MCP Apps Server's
versioned reference is already hosted on
[docs.rs](https://docs.rs/mcp-apps-server/0.1.0-alpha.1/mcp_apps_server/).

The generated reference is stored in `rust/target/doc/`, outside Git. Its files
are useful after a local build; the source links above work when browsing the
repository on GitHub.

## Contributing

<details>
<summary>Run the contributor checks</summary>

From `rust/`:

```sh
cargo fmt --all --check
cargo clippy --locked -p openai-mcp-extensions --no-default-features --all-targets -- -D warnings
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked -p openai-mcp-extensions --no-default-features
cargo check --locked -p openai-mcp-extensions --all-targets --no-default-features --features derive
cargo check --locked -p openai-mcp-extensions --all-targets --no-default-features --features mcp-apps
cargo doc --locked -p openai-mcp-extensions --no-default-features --no-deps
cargo doc --locked --workspace --all-features --no-deps
```

Set `RUSTDOCFLAGS="-D warnings"` for a strict documentation build. CI includes
Linux, Windows and the declared minimum Rust version. Core and combined-feature
tests exercise behavior; the independent feature builds detect accidental
coupling. The two app examples share protocol assertions for capabilities, tool
associations, resource contents and ordinary tool results. These in-memory
exchanges do not establish host UI rendering or permissions.

`Cargo.lock` makes contributor checks reproducible; consumers resolve dependencies
through their own lockfiles.

</details>

<details>
<summary>Verify distributable crate archives</summary>

From `rust/`, verify both SDK packages before publication:

```sh
cargo package --locked --workspace --all-features --allow-dirty --config 'patch.crates-io.openai-mcp-extensions-macros.path="derive"'
```

Cargo builds each archive with its normalized manifest. The patch supplies the
SDK's own matching macro package until it is published; MCP Apps Server resolves
from crates.io. `--allow-dirty` permits checking uncommitted changes and does not
publish anything. Review the package file lists before a release.

CI also rebuilds the archived SDK against the archived SDK macros. Publish
`openai-mcp-extensions-macros` before `openai-mcp-extensions`; the external MCP
Apps runtime and its optional macros are already available from crates.io.

> **TODO(release):** Verify both package READMEs in their published context. Links
> to the specification, repository examples and the other SDK package leave the
> individual crate archives; supplement them with their actual public source or
> documentation URLs before publication.

</details>

<details>
<summary>Protocol and host compatibility</summary>

The local specification is the protocol baseline. Rendering, uploads, app launches
and identifier availability depend on host capabilities. SDK-specific support for
quick actions/settings entrypoints and additional primitive constraints is described
in the API reference; declarations do not establish host implementation.

</details>

## License

[Apache License 2.0](LICENSE).
