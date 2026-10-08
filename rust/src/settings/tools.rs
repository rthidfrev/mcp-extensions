//! Standalone validated settings tools.
use super::{SettingsCapability, SettingsRegistration, wire};
use crate::Error;
use crate::extensions::ExtensionFuture;
use crate::extensions::tool::{internal, make_tool, structured};
use rmcp::{
    ErrorData, RoleServer, handler::server::router::tool::ToolRoute, model::JsonObject,
    service::RequestContext,
};
use serde_json::Value;
use std::{future::Future, sync::Arc};

/// A settings read/update pair ready for installation in a consumer registry.
///
/// Construction installs nothing. Both routes share the validated registration;
/// invoke their `call` fields after consumer checks to retain SDK validation.
/// Install both tools atomically before advertising the capability. Persistence,
/// authorization and diagnostics remain the consumer's responsibilities.
#[derive(Debug)]
pub struct SettingsTools<S> {
    read: ToolRoute<S>,
    update: ToolRoute<S>,
    capability: SettingsCapability,
}

impl<S> SettingsTools<S> {
    /// The read route, including its complete schema and validated execution.
    pub fn read(&self) -> &ToolRoute<S> {
        &self.read
    }

    /// The update route, including patch and handler-result validation.
    pub fn update(&self) -> &ToolRoute<S> {
        &self.update
    }

    /// Inspect the capability description; this does not assert installation.
    pub fn capability(&self) -> &SettingsCapability {
        &self.capability
    }

    /// Take both routes and their capability description for custom installation.
    /// Preflight both names (including disabled/reserved entries), then commit the
    /// full pair or roll back before advertising. This SDK cannot inspect your registry.
    pub fn into_parts(self) -> ([ToolRoute<S>; 2], SettingsCapability) {
        ([self.read, self.update], self.capability)
    }
}

impl SettingsRegistration {
    /// Build validated read/update tools without choosing a server registry.
    ///
    /// `read` returns every effective value. `update` receives validated replacements
    /// and must authorize and persist them atomically before returning every value.
    /// Both receive the original request context. Invalid handler results are internal
    /// MCP errors; validation cannot undo application writes. No defaults are inserted.
    pub fn into_tools<S, R, U, RFut, UFut>(self, read: R, update: U) -> SettingsTools<S>
    where
        S: Send + Sync + 'static,
        R: Fn(RequestContext<RoleServer>) -> RFut + Send + Sync + 'static,
        U: Fn(JsonObject, RequestContext<RoleServer>) -> UFut + Send + Sync + 'static,
        RFut: Future<Output = Result<JsonObject, ErrorData>> + Send + 'static,
        UFut: Future<Output = Result<JsonObject, ErrorData>> + Send + 'static,
    {
        self.into_tools_with_state(
            move |_, context| Box::pin(read(context)),
            move |_, set, context| Box::pin(update(set, context)),
        )
    }

    /// Build settings tools whose callbacks may borrow the current server.
    ///
    /// Each callback receives the server from the original `ToolCallContext` and
    /// the unchanged request context. Return a boxed `Send` future tied to the
    /// server borrow; no additional consumer `Arc` is needed to use state after
    /// `.await`. Callbacks must be `Send + Sync + 'static`, the server `Send + Sync`.
    /// Validation and tool declarations are identical to [`Self::into_tools`]:
    /// input is checked before invoking a callback, and invalid returned values
    /// produce internal MCP errors after its effects. The consumer remains
    /// responsible for authorization, persistence and atomic pair installation.
    pub fn into_tools_with_state<S, R, U>(self, read: R, update: U) -> SettingsTools<S>
    where
        S: Send + Sync + 'static,
        R: for<'a> Fn(&'a S, RequestContext<RoleServer>) -> ExtensionFuture<'a, JsonObject>
            + Send
            + Sync
            + 'static,
        U: for<'a> Fn(
                &'a S,
                JsonObject,
                RequestContext<RoleServer>,
            ) -> ExtensionFuture<'a, JsonObject>
            + Send
            + Sync
            + 'static,
    {
        let registration = self;
        let capability = registration.capability.clone();
        let read_tool = make_tool(
            &capability.read_tool,
            wire::read_input_schema(),
            Some(wire::read_output_schema(&registration)),
            true,
        );
        let update_tool = make_tool(
            &capability.update_tool,
            wire::update_input_schema(&registration),
            Some(wire::update_output_schema(&registration)),
            false,
        );
        let registration = Arc::new(registration);
        let read_registration = registration.clone();
        let read = Arc::new(read);
        let read_route = ToolRoute::new_dyn(read_tool, move |context| {
            let registration = read_registration.clone();
            let read = read.clone();
            Box::pin(async move {
                if context.arguments.as_ref().is_some_and(|a| !a.is_empty()) {
                    return Err(ErrorData::invalid_params(
                        "settings read arguments must be empty",
                        None,
                    ));
                }
                let values = read(context.service, context.request_context).await?;
                registration.validate_values(&values).map_err(internal)?;
                let content = serde_json::to_value(wire::ReadResult {
                    schema: &registration.schema,
                    values: &values,
                    layout: registration.layout.as_deref(),
                })
                .map_err(|source| Error::from(source).into_internal())?;
                Ok(structured(content).into())
            })
        });
        let update = Arc::new(update);
        let update_route = ToolRoute::new_dyn(update_tool, move |context| {
            let registration = registration.clone();
            let update = update.clone();
            Box::pin(async move {
                let args: wire::UpdateArguments =
                    serde_json::from_value(Value::Object(context.arguments.unwrap_or_default()))
                        .map_err(|source| ErrorData::from(Error::from(source)))?;
                registration
                    .validate_patch(&args.set)
                    .map_err(ErrorData::from)?;
                let values = update(context.service, args.set, context.request_context).await?;
                registration.validate_values(&values).map_err(internal)?;
                let content = serde_json::to_value(wire::UpdateResult { values: &values })
                    .map_err(|source| Error::from(source).into_internal())?;
                Ok(structured(content).into())
            })
        });

        SettingsTools {
            read: read_route,
            update: update_route,
            capability,
        }
    }
}
