use crate::{
    Error,
    mentions::{self, MentionSearchParams, MentionSearchResult, SEARCH_MENTIONS_TOOL},
    settings::{
        SettingsCapability, SettingsModel, SettingsRegistration, SettingsTools, TypedSettings,
    },
};
use rmcp::{
    ErrorData, RoleServer,
    handler::server::router::tool::ToolRouter,
    model::{JsonObject, ServerCapabilities, ToolsCapability},
    service::RequestContext,
};
use std::{future::Future, pin::Pin};

pub(crate) mod tool;

/// A boxed `Send` callback future that may borrow the server for `'a`.
///
/// Use `Box::pin(async move { ... })` in a callback receiving `&'a S`.
/// This type erases the concrete future while preserving its borrow lifetime;
/// the result or MCP error is returned when the operation completes.
pub type ExtensionFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ErrorData>> + Send + 'a>>;

/// Installs OpenAI tools in an existing rmcp ToolRouter.
/// Configure before serving; advertise capabilities only after successful registration.
/// For a custom registry, use the standalone mentions and settings builders instead.
#[derive(Debug, Default)]
pub struct OpenAIExtensions {
    settings: Option<SettingsCapability>,
    has_tools: bool,
}

impl OpenAIExtensions {
    /// An empty extension registration; no tools are installed automatically.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build and install the standalone mention tool. Empty queries are valid.
    /// Enabled, disabled and pre-disabled names are conflicts. Failure leaves the
    /// router and capability state unchanged. The callback owns authorization.
    pub fn register_mentions<S, F, Fut>(
        &mut self,
        router: &mut ToolRouter<S>,
        handler: F,
    ) -> Result<(), Error>
    where
        S: Send + Sync + 'static,
        F: Fn(MentionSearchParams, RequestContext<RoleServer>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<MentionSearchResult, ErrorData>> + Send + 'static,
    {
        self.register_mentions_with_state(router, move |_, params, context| {
            Box::pin(handler(params, context))
        })
    }

    /// Install mention search with a callback borrowing the current server.
    ///
    /// The callback may use `&S` across `.await` in its boxed `Send` future.
    /// Declaration, validation, collision checks and capability updates match
    /// [`Self::register_mentions`]. The original request context is preserved;
    /// the callback owns authorization. Configure before serving.
    pub fn register_mentions_with_state<S, F>(
        &mut self,
        router: &mut ToolRouter<S>,
        handler: F,
    ) -> Result<(), Error>
    where
        S: Send + Sync + 'static,
        F: for<'a> Fn(
                &'a S,
                MentionSearchParams,
                RequestContext<RoleServer>,
            ) -> ExtensionFuture<'a, MentionSearchResult>
            + Send
            + Sync
            + 'static,
    {
        ensure_available(router, SEARCH_MENTIONS_TOOL)?;
        router.add_route(mentions::search_tool_with_state(handler));
        self.has_tools = true;
        Ok(())
    }

    /// Build and install the standalone validated settings pair atomically.
    /// Both names are checked, including disabled/pre-disabled names, before mutation.
    /// Failure leaves routes and capability state unchanged. Callbacks own
    /// authorization and persistence; SDK result validation cannot undo writes.
    pub fn register_settings<S, R, U, RFut, UFut>(
        &mut self,
        router: &mut ToolRouter<S>,
        registration: SettingsRegistration,
        read: R,
        update: U,
    ) -> Result<(), Error>
    where
        S: Send + Sync + 'static,
        R: Fn(RequestContext<RoleServer>) -> RFut + Send + Sync + 'static,
        U: Fn(JsonObject, RequestContext<RoleServer>) -> UFut + Send + Sync + 'static,
        RFut: Future<Output = Result<JsonObject, ErrorData>> + Send + 'static,
        UFut: Future<Output = Result<JsonObject, ErrorData>> + Send + 'static,
    {
        self.register_settings_with_state(
            router,
            registration,
            move |_, context| Box::pin(read(context)),
            move |_, set, context| Box::pin(update(set, context)),
        )
    }

    /// Install a settings pair with callbacks borrowing the current server.
    ///
    /// The boxed `Send` futures may use `&S` across `.await`, and receive the
    /// original request context. Validation, errors, collision checks and atomic
    /// capability registration match [`Self::register_settings`]. Callbacks own
    /// authorization and persistence; output validation cannot undo their effects.
    pub fn register_settings_with_state<S, R, U>(
        &mut self,
        router: &mut ToolRouter<S>,
        registration: SettingsRegistration,
        read: R,
        update: U,
    ) -> Result<(), Error>
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
        self.install_settings_tools(router, registration.into_tools_with_state(read, update))
    }

    /// Install typed settings using the same routes and atomic name preflight as
    /// raw settings. Callbacks own authorization and persistence; incomplete or
    /// invalid returned values are internal errors after application effects.
    pub fn register_typed_settings<S, V, R, U, RFut, UFut>(
        &mut self,
        router: &mut ToolRouter<S>,
        registration: TypedSettings<V>,
        read: R,
        update: U,
    ) -> Result<(), Error>
    where
        S: Send + Sync + 'static,
        V: SettingsModel,
        R: Fn(RequestContext<RoleServer>) -> RFut + Send + Sync + 'static,
        U: Fn(V::Patch, RequestContext<RoleServer>) -> UFut + Send + Sync + 'static,
        RFut: Future<Output = Result<V, ErrorData>> + Send + 'static,
        UFut: Future<Output = Result<V, ErrorData>> + Send + 'static,
    {
        self.install_settings_tools(router, registration.into_tools(read, update))
    }

    /// Install typed settings whose callbacks borrow the original server across
    /// await. Return `Box::pin(async move { ... })`; validation, collision checks
    /// and capability advertisement match [`Self::register_typed_settings`].
    pub fn register_typed_settings_with_state<S, V, R, U>(
        &mut self,
        router: &mut ToolRouter<S>,
        registration: TypedSettings<V>,
        read: R,
        update: U,
    ) -> Result<(), Error>
    where
        S: Send + Sync + 'static,
        V: SettingsModel,
        R: for<'a> Fn(&'a S, RequestContext<RoleServer>) -> ExtensionFuture<'a, V>
            + Send
            + Sync
            + 'static,
        U: for<'a> Fn(&'a S, V::Patch, RequestContext<RoleServer>) -> ExtensionFuture<'a, V>
            + Send
            + Sync
            + 'static,
    {
        self.install_settings_tools(router, registration.into_tools_with_state(read, update))
    }

    fn install_settings_tools<S>(
        &mut self,
        router: &mut ToolRouter<S>,
        tools: SettingsTools<S>,
    ) -> Result<(), Error>
    where
        S: Send + Sync + 'static,
    {
        if self.settings.is_some() {
            return Err(crate::error::invalid("settings already registered"));
        }
        for name in [
            &tools.capability().read_tool,
            &tools.capability().update_tool,
        ] {
            ensure_available(router, name)?;
        }
        let ([read, update], capability) = tools.into_parts();
        router.add_route(read);
        router.add_route(update);
        self.settings = Some(capability);
        self.has_tools = true;
        Ok(())
    }

    /// Advertise successfully installed tools/settings, preserving unrelated namespaces.
    /// Settings use both modern `extensions` and legacy `experimental` maps.
    /// Configure before serving; this snapshot cannot observe later external route
    /// removal, disabling or replacement. Keep it consistent with your live registry.
    pub fn apply_capabilities(&self, capabilities: &mut ServerCapabilities) {
        if self.has_tools && capabilities.tools.is_none() {
            capabilities.tools = Some(ToolsCapability::default());
        }
        if let Some(settings) = &self.settings {
            settings.apply_capabilities(capabilities);
        }
    }
}

fn ensure_available<S>(router: &ToolRouter<S>, name: &str) -> Result<(), Error>
where
    S: Send + Sync + 'static,
{
    if router.map.contains_key(name) {
        return Err(Error::ToolCollision(name.into()));
    }
    // rmcp has no query for a pre-disabled name without a registered route.
    // Probe a detached clone, without notification effects or changes to the caller.
    let mut probe = router.clone();
    probe.clear_notifier();
    if probe.enable_route(name) {
        return Err(Error::ToolCollision(name.into()));
    }
    Ok(())
}
