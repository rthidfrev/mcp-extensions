# OpenAI MCP Extensions for Rust

Server-side [OpenAI MCP Extensions](../docs/spec.md) for the
[official Rust MCP SDK, `rmcp`](https://github.com/modelcontextprotocol/rust-sdk).
The crate adds mention search, native settings, MCP App metadata, resource input
helpers and extended form elicitation to an existing server.

## Installation

Requires Rust 1.88 or later and `rmcp` 3.5. This crate is currently available from
this repository; it has not been published to crates.io.

From a checkout, use a path dependency pointing to the `rust` directory:

```toml
[dependencies]
openai-mcp-extensions = { path = "../mcp-extensions/rust" }
rmcp = { version = "3.5.0", features = ["server", "macros"] }
serde_json = "1"
```

The library enables rmcp's server support. Enable your chosen transport on your
application's rmcp dependency. The crate provides no browser runtime, persistence
layer or HTTP framework.

## Integrate with an existing server

Create one `OpenAIExtensions` alongside your `ToolRouter`, register extensions
before the server starts, and keep both in your server. In `ServerHandler::get_info`,
call `extensions.apply_capabilities(&mut config.capabilities)`. This preserves other
capability namespaces and advertises installed settings tools in both `extensions`
and the legacy `experimental` map.

Keep your existing `list_tools` and `call_tool` methods routing through this same
`ToolRouter`. rmcp handles discovery, initialization, cancellation and transport.
The SDK installs extension routes without replacing your server handler.
An empty registration installs no tools. Registering over an existing tool name
fails before changing the router. Register all other tools before the extensions,
and do not subsequently replace their routes using rmcp's `add_route`.

### Composer mentions

```rust
use openai_mcp_extensions::{OpenAIExtensions, mentions::MentionSearchResult};
use rmcp::handler::server::router::tool::ToolRouter;

let mut router: ToolRouter<()> = ToolRouter::new();
let mut extensions = OpenAIExtensions::new();
extensions.register_mentions(&mut router, |params, context| async move {
    // Authorize this caller using context, then search for params.query.
    Ok(MentionSearchResult { items: vec![] })
})?;
```

This installs `search_mentions` with a read-only annotation, the
`openai/extensions` mention marker, and MCP App visibility. An empty query is
valid. Responses have empty `content` and an `items` array in `structuredContent`.
Return `MentionItem::ResourceLink` with an rmcp `Resource` for the standard
`resource_link` shape. `MentionItem::Resource` also accepts the `resourceUri`/`title`
presentation supported by the TypeScript and Python SDKs.

### Native settings

`SettingsRegistration::new` accepts a flat JSON Schema object. Each field needs a
nonblank `title` and a boolean, string, number or integer type. String enums and
ordinary primitive constraints are supported; nested objects, arrays, references,
schema composition and schema defaults are rejected.

```rust
use openai_mcp_extensions::settings::SettingsRegistration;
use serde_json::json;

let settings = SettingsRegistration::new(json!({
    "type": "object",
    "properties": {
        "units": {"type": "string", "title": "Units", "enum": ["mm", "in"]},
        "showGrid": {"type": "boolean", "title": "Show grid"}
    },
    "required": ["units", "showGrid"]
}))?;
```

Pass this registration and two asynchronous handlers to
`extensions.register_settings(&mut router, settings, read, update)`. Both receive
the rmcp request context for authorization. `read` returns all effective values.
`update` additionally receives a validated, nonempty `set` map of replacements.
It must authorize the caller, persist those replacements atomically, and then
return all effective values. Omitted fields keep their previous values; there is
no deep merge or reset operation.

The SDK validates input before invoking `update` and validates both handlers'
results against every schema property, including optional fields. It does not
insert defaults or persist values. Invalid handler results produce internal MCP
errors; they do not roll back application writes. Persistence and concurrent
updates remain the application's responsibility.

The read tool's `outputSchema` describes the registered schema and, when supplied,
the exact ordered layout, as well as all effective values. This registration is
immutable: change it before serving rather than returning a different schema from
a handler. Settings also accept `format` and exclusive numeric bounds supported by
the validator; these extend the pinned OpenAI settings subset and host rendering
of those extra constraints is not established.

`with_tool_names` changes the default `settings.read` / `settings.update` names.
`with_layout` adds ordered `SettingsGroup` sections containing property controls
or same-server tool buttons. Unknown and duplicate property references are
rejected. The application supplies and authorizes any referenced button tools.

### MCP App metadata and resources

Apply `UiToolMetadata` or `UiResourceMetadata` to an rmcp `MetaObject`. Their
`apply` methods validate and replace `openai/ui`, preserving other namespaces
such as the MCP App's `ui.resourceUri`. Tool entrypoints support file, global,
settings and thread launches, including global quick actions. OpenAI resource
display modes are `inline` and `fullscreen`. `DisplayMode::Pip` remains available
for source compatibility with base MCP Apps, but `UiResourceMetadata::apply`
rejects it before modifying metadata. Omitting both fields leaves the host's
default of inline/fullscreen; a sole preference defaults the available modes to
that preference. The host may ignore the preference.

```rust
use openai_mcp_extensions::ui::{associate_app_resource, app_html_resource};
use rmcp::model::MetaObject;
use serde_json::json;

let mut tool_meta = MetaObject::default();
associate_app_resource(&mut tool_meta, "ui://parts/viewer")?;
// Assign tool_meta to your Tool's meta and register its ordinary rmcp route.
let resource_meta = MetaObject(json!({
    "ui": {"csp": {"connectDomains": [], "resourceDomains": []}}
}).as_object().unwrap().clone());
let contents = app_html_resource(
    "ui://parts/viewer", "<!doctype html><main>Parts</main>", resource_meta,
)?;
// Return these contents from ServerHandler::read_resource.
```

`associate_app_resource` preserves existing `ui.visibility` and other namespaces.
`app_html_resource` emits `text/html;profile=mcp-app` and preserves content metadata,
including the host-enforced `ui.csp` security declarations. The application owns
the HTML and any JavaScript bundle, resource registration and authorization.
`supports_mcp_apps` checks the caller's declared HTML MIME support. Hosts without
Apps support can still use ordinary tool results; supply a useful text fallback.
These helpers do not enforce iframe CSP or implement the browser-side Apps SDK.

Global/thread entrypoint tools and settings buttons must accept `{}`. File tools
accept the input below. Use distinct, descriptive thread titles such as `Parts
Tray`, and native rmcp tool/server icons. SVGs should use `currentColor` and a square
viewport; icon/title fallbacks are chosen by the host. Quick actions, settings
entrypoints and model-preferred modes follow the reference SDKs and exceed the
pinned entrypoint spec, so their presence does not establish host support.

`FileEntrypointInput` decodes `{ "file": { "name", "resourceUri" } }`; call
`validate` after deserializing. `resource_tool_call_metadata(&context.meta)` reads
the host's `openai/resource.path`, returning `None` when absent and an error when
malformed. The path belongs to the host. These helpers never read a local file.
Treat that path as untrusted input: the application authorizes any access, and a
remote server may not share the host's filesystem. `requested_representation`
decodes optional `openai/resource.representation` into `Text` or `Blob`. An ordinary
server-owned resource may honor it; use rmcp text contents or base64 blob contents.
The host intercepts file-entrypoint resource reads. Browser read/write/subscribe
methods remain application/host responsibilities.

### Extended forms

```rust
use openai_mcp_extensions::forms::{OpenAIForm, elicit_input};
use serde_json::json;

let form = OpenAIForm::from_json(json!({
    "type": "object",
    "properties": {
        "code": {
            "type": "string",
            "pattern": "^[A-Z]{2}$",
            "x-openai-suggestions": [{"const": "AB", "title": "Example"}]
        }
    },
    "required": ["code"]
}))?;
// Inside a tool handler with an rmcp RequestContext<RoleServer>:
let answer = elicit_input(&context, "Enter a part code", &form).await?;
```

Forms retain their JSON schema without converting through rmcp's narrower
`ElicitationSchema`. They support primitive fields, string enums and titled
choices, string arrays, patterns, suggestions, option descriptions, thumbnail
annotations, and URI resource selection. `FormField` and `ResourceInput` provide
field builders; raw JSON also supports the full documented subset. Resource
options retain native MCP metadata, including `openai/thumbnail` and
`openai/preview`. Legacy `file` input and option `x-openai-preview` spellings are
accepted.

Titled-choice thumbnails must use HTTPS or a base64 image data URI. Sources are
checked for URI syntax, an HTTPS host or a nonempty base64 image payload; the SDK
does not fetch URLs or decode image formats. For resource previews, use
`PreviewTarget::McpAppTool` or `PreviewTarget::ResourceLink` and apply the target to
the option's metadata. Raw `openai/preview.target` is validated by the same rules
without stripping unrelated metadata. Tool names must be nonblank and linked
resources have a name and absolute URI. The host resolves the target; the SDK does
not execute preview tools while constructing a form.
Resource-option `openai/thumbnail` follows the delegated MCP `Icon` shape; the
spec's explicit HTTPS/base64-image source restriction concerns titled choices.

Unknown schema keywords and unsupported semantic inputs are rejected instead of
discarded. Validation uses JSON Schema Draft 2020-12, including format checks,
plus resource-selection rules. Defaults must be valid; resource defaults must
name supplied options. Implicit selection cannot specify a default; single
selection cannot specify a selection mode. Accepted answers are validated
without coercion or default insertion. Suggestions are hints, not enum constraints.
Explicit selection offers user uploads only when `userOptions` is provided;
implicit selection with omitted `userOptions` has the host-defined file-picker
default. Web supports explicit selection, subject to the spec's upload limitation.
Here, web means the Work browser; the spec's support table excludes classic ChatGPT.

`elicit_input` checks the caller's `openai/elicitation.form` capability and sends
`openai/elicitation/create`. It returns `FormResult::Accept`, `Cancel` or `Decline`.
Use `elicit_input_with_options` for rmcp timeout options.

The direct helper requires a legacy connection, such as MCP `2025-11-25`. It
rejects modern requests before sending. MCP `2026-07-28` or later uses
[multi-round-trip requests (MRTR)](https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/mrtr):
a tool returns a keyed input request and opaque `requestState`; the host retries
the same tool with its original arguments, that state and keyed `inputResponses`.
OpenAI-registered servers require MRTR. Direct connections support both flows.

### Modern form round trips

Wrap your existing rmcp server with `forms::mrtr::FormService` before calling
`serve`. Register modern callbacks using `register(tool_name, callback)`. These
callbacks receive the original `CallToolRequestParams` and request context and
return `FormToolResponse`. Declare the same tools in the underlying server's
`tools/list` and keep their legacy routes there. The adapter intercepts only
registered modern form calls; it delegates ordinary requests, notifications,
discovery and legacy calls to the existing service.

Use the same validated `OpenAIForm` on both flows. A modern callback can request
input with:

```rust
use openai_mcp_extensions::forms::mrtr::FormInputRequired;

// `state` is an opaque handle issued by the application; `form` is an OpenAIForm.
let input = FormInputRequired::new(state, "answer", "Enter a part code", &form)?;
// Return Ok(input.into()) from the registered modern callback.
```

The outgoing result has `resultType: "input_required"` and embeds
`openai/elicitation/create`, preserving every validated rich-schema field. The
host supplies a bare `{ "action": "accept", "content": ... }`, `decline` or
`cancel` result under `inputResponses.answer`. `read_input(&params, "answer",
&form)` returns `FormInput::Initial` or a validated `FormInput::Response`. It
rejects missing state/answers and invalid accepted content without coercion or
default insertion. `add_form` supports distinct keys for multiple forms;
validate the complete expected set before applying an effect. Terminal handlers
return `CallToolResult` through `FormToolResponse::Complete`.

The application owns continuation storage and its security. Bind saved state to
the authorized caller, original arguments and form definitions; check integrity
and expiry, and consume it atomically with any application write. An echoed state
handle is not authentication. The SDK neither persists nor consumes it. Callbacks
receive rmcp's cancellation token and must check it before effects. Handle decline
and cancel explicitly rather than treating them as accepted input.

Every intercepted modern request must contain valid `_meta.protocolVersion` and
`_meta.clientCapabilities`, including `extensions["openai/elicitation"].form`.
`clientInfo` is optional and is not an authenticated identity. Unsupported
versions, malformed required metadata and missing capabilities produce protocol
errors before the callback. Modern results include the underlying server's
implementation information and preserve other result metadata. The adapter
offers no task-result variant.

rmcp's standard embedded-request union does not represent the OpenAI method.
The adapter uses rmcp's public `Service` and `CustomResult` APIs to retain the
complete result without modifying rmcp. JSON-RPC byte tests verify this contract
and failure paths. A separate Codex direct-connection experiment established the
embedded method and rich thumbnail display; it did not run this Rust server.
The shipped Rust server subsequently completed a native Codex Desktop
`2026-07-28` trial: `choose_part` returned an OpenAI form, Codex retried with
the accepted selection and unchanged state, and Rust returned the final result.
`settings.read` also returned its schema, layout and effective values through
Codex. This bounded trial does not establish native settings UI rendering,
thumbnail loading or compatibility with other hosts.
Actual deployment through the OpenAI server registry has not been exercised.

## Runnable example

From the repository root:

```sh
cargo run --manifest-path rust/Cargo.toml --example server
```

[The stdio example](examples/server.rs) exposes `search_mentions`, native settings,
`choose_part` and `view_parts`, with ordinary `resources/list` and `resources/read`.
An MCP host launches the process and exchanges JSON-RPC over stdin/stdout. It
supports legacy `2025-11-25` initialization, modern `2026-07-28` discovery and both
form flows. Modern part choices use a bounded in-memory store, ten-minute expiry
and single-use sequence handles. These handles suit this public read-only demo;
they provide no authentication or protection for private or mutating tools.
Settings are stored in memory for that process. The parts tool declares
global/thread/file entrypoints and accepts both
`{}` and file input. Its HTML is a static server-resource illustration, without
browser initialization or result handling; replace it with your bundled MCP App.
Its catalog resource demonstrates text/blob hints. Image URLs are illustrative;
replace them with your own assets. Rendering requires an appropriate host and UI.

## Server conformance boundaries

The protocol reference is this checkout's [extension specification](../docs/spec.md)
at OpenAI commit `900032d8bd7c1566202d0cb1666986584f932043`. UI server declarations
follow the MCP Apps revision linked there; its dated display-mode section narrows
OpenAI modes to inline/fullscreen. Standard MCP mechanics are delegated to rmcp.

| Server responsibility                                           | Implementation and evidence                                                                                                                            | Remaining boundary                                                                        |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------- |
| Discovery, capabilities, ordinary tools/resources               | rmcp integration; legacy initialization and modern discovery exercised over a byte transport; native Codex discovery and form/settings tools exercised | Other ordinary tool/resource workflows in the actual host remain unverified               |
| Entrypoints, UI association, titles/icons, display declarations | Validated OpenAI metadata and native rmcp fields; example launch with `{}`/file input and resource MIME/security metadata                              | Host launch behavior, rendering, icon fallbacks and modal behavior are host-owned         |
| Mentions and structured settings                                | Registered routes, result validation, partial updates and full effective values; schema/layout and mutation-boundary checks                            | The application owns authorization, persistence, concurrency and referenced button tools  |
| Host path and ordinary read representation                      | Metadata decoders and runnable text/blob example                                                                                                       | No filesystem permission or browser file API is implemented                               |
| Rich forms, previews, thumbnails and answers                    | Lossless schemas, URI/default/selection rules, validated previews and sources; direct-flow byte exchanges                                              | Rendering, uploads and previews require the matching host capabilities                    |
| Modern rich-form MRTR                                           | Public rmcp service adapter; lossless embedded requests and validated retries; missing-capability, invalid-answer and replay checks                    | Application-owned secure continuation state; actual OpenAI-registered deployment untested |
| Plugin onboarding                                               | Manifest convention shown below; no Rust runtime handler is needed                                                                                     | Packaging and onboarding execution belong to the plugin and host                          |

Plugin onboarding uses the ordinary plugin manifest, independently of the server
language. For example, the following fragment points to a skill in the plugin
archive, relative to its root:

```json
{
  "extensions": {
    "com.openai": {
      "onboardingSkill": "skills/onboarding/SKILL.md"
    }
  }
}
```

Include that file in the plugin archive; this crate's Cargo archive does not create
a plugin package. See the existing [Bits & Bolts plugin](../plugins/bits-and-bolts/README.md)
and [onboarding skill](../plugins/bits-and-bolts/skills/onboarding/SKILL.md).

## Development and validation

Run from `rust/`:

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo check --locked --lib --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
cargo package --locked --allow-dirty
```

`Cargo.lock` is retained for reproducible contributor and CI checks. Consumers
resolve dependencies through their own lockfiles. Archive verification builds
the packaged library and excludes build outputs and local working files.
CI runs Rust checks on Linux and Windows and checks the minimum Rust version.
Rust publication is not configured in the existing Python/Node release workflow;
it requires a separate maintainer decision.

Contract tests cover schemas, metadata serialization and failure cases.
Integration tests connect an rmcp server and client over an in-memory byte
transport and exercise discovery, settings persistence boundaries and both form
flows. Modern tests use a raw JSON-RPC peer so rmcp's standard embedded-request
types cannot strip the OpenAI method or schema. They verify MCP wire behavior;
they do not prove that ChatGPT or Codex
renders an app or form. Browser-side MCP App APIs remain available through the
[TypeScript SDK](../typescript/README.md).

The shipped example is also tested directly on both protocol profiles. New checks
target established source/preview defects, metadata preservation and ordinary
server integration. Test counts and line coverage are not completeness criteria.

## License

[Apache License 2.0](LICENSE).
