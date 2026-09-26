use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{App, AppContext as _, Entity, Global, SharedString, Window};
use gpui_shell::{ComponentCallback, HostModule, ScriptView, ShellRuntime, policy::Policy};

use super::{
    Catalog, CommandId, CommandMode, Extension, ExtensionCommand, LaunchRequest, bridge,
    lifecycle::{BACKGROUND_LIMIT, IdleTicket, KEEP_ALIVE, LaunchId, Lifecycle},
    pages::{ArgumentsPage, ExtensionsPage, PermissionPage, PreferencesPage},
    paths::DataDirectory,
    permissions::{PermissionState, PermissionStore, RequestedCapabilities},
    preferences::{
        KeychainSecrets, PreferenceStore, ResolvedPreferences, SecretStore, all_scopes, scopes,
    },
};
use crate::{
    model::{Effect, Toast},
    pages::{self, PageHandle, ScriptPage},
};

/// What opening a command produced.
pub enum Opened {
    /// A page to push: the command's own, or a page the launcher asks
    /// something on first (permissions, preferences, arguments).
    Page(PageHandle),
    /// A no-view command that runs without a page.
    ///
    /// The window stays as it is: a no-view command reports back with a
    /// toast, which needs the window, or with a HUD (`show_hud`), which hides
    /// it — the same choice Raycast leaves to the command.
    Background,
}

type EffectHandler = Rc<dyn Fn(Effect, &mut App)>;

/// Where effects requested by extensions and by the launcher's own extension
/// pages go; the launcher window performs them.
#[derive(Clone, Default)]
pub(super) struct EffectSink(Rc<RefCell<Option<EffectHandler>>>);

impl EffectSink {
    fn set(&self, handler: impl Fn(Effect, &mut App) + 'static) {
        self.0.replace(Some(Rc::new(handler)));
    }

    pub(super) fn request(&self, effect: Effect, cx: &mut App) {
        let Some(handler) = self.0.borrow().clone() else {
            tracing::info!("effect requested with no launcher window: {effect:?}");
            return;
        };
        handler(effect, cx);
    }

    pub(super) fn toast(&self, toast: Toast, cx: &mut App) {
        self.request(Effect::ShowToast(toast), cx);
    }
}

/// Called after an extension was installed, updated or removed, so the
/// window can read the catalog again.
#[derive(Clone, Default)]
pub(super) struct ChangeNotifier(Rc<RefCell<Option<Rc<dyn Fn(&mut App)>>>>);

impl ChangeNotifier {
    pub(super) fn notify(&self, cx: &mut App) {
        let handler = self.0.borrow().clone();
        if let Some(handler) = handler {
            handler(cx);
        }
    }
}

/// Where an extension's `update_command_metadata` goes: the root search shows
/// the new subtitle in place of the manifest's.
#[derive(Clone, Default)]
pub(super) struct MetadataNotifier(
    Rc<RefCell<Option<Rc<dyn Fn(CommandId, bridge::CommandMetadata, &mut App)>>>>,
);

impl MetadataNotifier {
    fn notify(&self, command: CommandId, update: bridge::CommandMetadata, cx: &mut App) {
        let handler = self.0.borrow().clone();
        if let Some(handler) = handler {
            handler(command, update, cx);
        }
    }
}

/// What the launcher's own extension pages work with.
#[derive(Clone)]
pub(super) struct Services {
    pub(super) data: DataDirectory,
    pub(super) permissions: PermissionStore,
    pub(super) preferences: PreferenceStore,
    pub(super) effects: EffectSink,
    pub(super) changed: ChangeNotifier,
    pub(super) metadata: MetadataNotifier,
    /// Directories whose extensions are being developed; `environment()`
    /// reports it to their code.
    pub(super) development: RefCell<Vec<PathBuf>>,
}

/// Everything a command is opened with, and what `launch()` answers.
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchContext {
    command: CommandId,
    arguments: BTreeMap<SharedString, SharedString>,
    preferences: ResolvedPreferences,
    data_dir: PathBuf,
    cache_dir: PathBuf,
    development: bool,
}

impl LaunchContext {
    pub fn command(&self) -> &CommandId {
        &self.command
    }

    pub fn arguments(&self) -> &BTreeMap<SharedString, SharedString> {
        &self.arguments
    }

    /// Resolved values, defaults applied, secrets included.
    pub fn preferences(&self) -> &ResolvedPreferences {
        &self.preferences
    }

    /// The extension's `${dataDir}`.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

/// How a launch's host functions reach the launcher.
#[derive(Clone)]
struct LaunchSink {
    effects: EffectSink,
    state: Weak<HostState>,
    launch: LaunchId,
}

impl LaunchSink {
    fn request(&self, effect: Effect, cx: &mut App) {
        self.effects.request(effect, cx);
    }

    /// The command said it is done (a HUD, closing the window). Releasing its
    /// view waits until the script call that said so has returned.
    fn finished(&self, cx: &mut App) {
        let (state, launch) = (self.state.clone(), self.launch);
        cx.defer(move |cx| {
            if let Some(state) = state.upgrade() {
                state.finish_background(launch, cx);
            }
        });
    }

    fn metadata(&self, command: CommandId, update: bridge::CommandMetadata, cx: &mut App) {
        if let Some(state) = self.state.upgrade() {
            state.services.metadata.notify(command, update, cx);
        }
    }
}

/// The HostModules a launch's policy grants: the only place that decides
/// what an extension can import besides the runtime's own modules.
///
/// `launcher/api` is built per launch, so `launch()` answers for the command
/// whose code is calling at any time — in `init`, in `render`, in a callback
/// three seconds later — rather than for whichever command was opened last.
fn host_modules(context: &LaunchContext, sink: &LaunchSink) -> Vec<HostModule> {
    let sink = sink.clone();
    let metadata = sink.clone();
    let api = context
        .arguments()
        .iter()
        .fold(
            bridge::ExtensionContext::new(context.command().extension().clone())
                .with_command(context.command().command().clone()),
            |api, (name, value)| api.with_argument(name.clone(), value.clone()),
        )
        .with_preferences(context.preferences().clone().into_iter().collect())
        .with_cache_directory(context.cache_dir())
        .with_development(context.development)
        .with_locale(locale())
        .with_effect_sink(move |effect, cx| {
            // A HUD or closing the window is how a no-view command says it
            // is done.
            let finishes = matches!(effect, Effect::ShowHud(_) | Effect::CloseWindow);
            sink.request(effect, cx);
            if finishes {
                sink.finished(cx);
            }
        })
        .with_metadata_sink(move |command, update, cx| metadata.metadata(command, update, cx));
    vec![
        bridge::HostApi::module_for(api),
        HostModule::source(bridge::UTILS_MODULE, bridge::utils_module_source())
            .declarations(bridge::utils_declarations()),
    ]
}

/// One opening of a command: the policy its code runs under and its view.
struct Launch {
    context: LaunchContext,
    /// The approved capability keys the policy was built with; a launch is
    /// reused only under the same grant.
    grant: BTreeSet<String>,
    mode: CommandMode,
    /// Also held by the launch's views; kept here so the application stays
    /// loaded while it is idle.
    policy: Rc<Policy>,
    view: Entity<ScriptView>,
}

struct HostState {
    services: Services,
    lifecycle: RefCell<Lifecycle>,
    launches: RefCell<BTreeMap<LaunchId, Launch>>,
}

impl HostState {
    /// Counts `page` as a user of `launch` until it is released.
    fn track_page(self: &Rc<Self>, launch: LaunchId, page: &Entity<ScriptPage>, cx: &mut App) {
        self.lifecycle.borrow_mut().acquire(launch);
        let state = Rc::downgrade(self);
        cx.observe_release(page, move |_, cx| {
            if let Some(state) = state.upgrade() {
                state.release_user(launch, cx);
            }
        })
        .detach();
    }

    fn release_user(self: &Rc<Self>, launch: LaunchId, cx: &mut App) {
        let Some(ticket) = self.lifecycle.borrow_mut().release_user(launch) else {
            return;
        };
        let state = Rc::downgrade(self);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(KEEP_ALIVE).await;
            cx.update(|_| {
                if let Some(state) = state.upgrade() {
                    state.expire(ticket);
                }
            });
        })
        .detach();
    }

    fn expire(&self, ticket: IdleTicket) {
        if self.lifecycle.borrow_mut().expire(ticket) {
            self.remove(ticket.launch());
        }
    }

    /// Drops a launch's view and policy: GPUI Shell cancels the tasks its
    /// application started.
    fn remove(&self, launch: LaunchId) {
        self.lifecycle.borrow_mut().remove(launch);
        let removed = self.launches.borrow_mut().remove(&launch);
        drop(removed);
    }

    /// Ends a no-view command's run.
    fn finish_background(&self, launch: LaunchId, _: &mut App) {
        let is_background = self
            .launches
            .borrow()
            .get(&launch)
            .is_some_and(|launch| launch.mode == CommandMode::NoView);
        if is_background {
            self.remove(launch);
        }
    }

    /// Releases the idle launches of an extension before another starts.
    ///
    /// Each launch holds its own `localStorage` cache over the extension's one
    /// file; an idle launch reused after another one wrote the file would
    /// answer from a stale cache and could write it back.
    fn release_idle(&self, extension: &SharedString) {
        let idle: Vec<LaunchId> = {
            let lifecycle = self.lifecycle.borrow();
            self.launches
                .borrow()
                .iter()
                .filter(|(id, launch)| {
                    launch.context.command.extension() == extension && lifecycle.is_idle(**id)
                })
                .map(|(id, _)| *id)
                .collect()
        };
        for launch in idle {
            self.remove(launch);
        }
    }

    /// An idle view-command launch opened with exactly this context and grant.
    fn reusable(&self, context: &LaunchContext, grant: &BTreeSet<String>) -> Option<LaunchId> {
        let lifecycle = self.lifecycle.borrow();
        self.launches
            .borrow()
            .iter()
            .find(|(id, launch)| {
                launch.mode == CommandMode::View
                    && launch.context == *context
                    && launch.grant == *grant
                    && lifecycle.is_idle(**id)
            })
            .map(|(id, _)| *id)
    }

    /// The launch whose policy `policy` is.
    fn launch_for(&self, policy: &Rc<Policy>) -> Option<LaunchId> {
        self.launches
            .borrow()
            .iter()
            .find(|(_, launch)| Rc::ptr_eq(&launch.policy, policy))
            .map(|(id, _)| *id)
    }
}

/// Lets [`page_from_callback`], which the bridge calls without a host at
/// hand, count a pushed page towards its launch.
struct HostRegistry(Weak<HostState>);

impl Global for HostRegistry {}

/// Runs extension commands on one shared GPUI Shell runtime, each launch
/// under a [`Policy`] of its own: the extension's approved grant, its storage,
/// and a `launcher/api` that knows which command it is serving.
pub struct ExtensionHost {
    runtime: Rc<ShellRuntime>,
    state: Rc<HostState>,
}

impl ExtensionHost {
    /// Creates the runtime, keeping data in the platform's data directory and
    /// secrets in the system keychain. Call once, after `gpui_shell::init`.
    pub fn new(cx: &mut App) -> Result<Self> {
        let data = DataDirectory::platform_default();
        let secrets = Rc::new(KeychainSecrets::new(data.secrets_file()));
        Self::new_in(data, secrets, cx)
    }

    /// Creates the runtime with its data in `data` and password preferences
    /// in `secrets`: a portable profile, or a test that must not touch the
    /// user's own.
    pub fn new_in(data: DataDirectory, secrets: Rc<dyn SecretStore>, cx: &mut App) -> Result<Self> {
        let runtime = ShellRuntime::new_with_components(cx, bridge::components()?)?;
        let services = Services {
            permissions: PermissionStore::new(data.permissions_file()),
            preferences: PreferenceStore::new(data.preferences_file(), secrets),
            data,
            effects: EffectSink::default(),
            changed: ChangeNotifier::default(),
            metadata: MetadataNotifier::default(),
            development: RefCell::default(),
        };
        let state = Rc::new(HostState {
            services,
            lifecycle: RefCell::new(Lifecycle::new()),
            launches: RefCell::new(BTreeMap::new()),
        });
        cx.set_global(HostRegistry(Rc::downgrade(&state)));
        Ok(Self { runtime, state })
    }

    /// The launcher's data directory. Its `extensions_dir()` holds the
    /// extensions installed from Git and belongs among the catalog roots.
    pub fn data(&self) -> &DataDirectory {
        &self.state.services.data
    }

    /// Where effects requested by extensions and by the extension pages go.
    pub fn set_effect_handler(&self, handler: impl Fn(Effect, &mut App) + 'static) {
        self.state.services.effects.set(handler);
    }

    /// Called after an extension was installed, updated or uninstalled, so
    /// the catalog can be read again.
    pub fn set_extensions_changed_handler(&self, handler: impl Fn(&mut App) + 'static) {
        self.state
            .services
            .changed
            .0
            .replace(Some(Rc::new(handler)));
    }

    /// The directories extensions are developed in, as `launcher dev` and the
    /// settings name them.
    pub fn set_development_directories(&self, directories: Vec<PathBuf>) {
        self.state.services.development.replace(directories);
    }

    /// Called when an extension updates a command's metadata, such as the
    /// subtitle the root search shows for it.
    pub fn set_metadata_handler(
        &self,
        handler: impl Fn(CommandId, bridge::CommandMetadata, &mut App) + 'static,
    ) {
        self.state
            .services
            .metadata
            .0
            .replace(Some(Rc::new(handler)));
    }

    /// Opens a command.
    ///
    /// Before the command's code runs, the launcher may need to ask: for
    /// permission, when the extension requests capabilities the user has not
    /// decided on; for required preferences; for required arguments. Each
    /// question is a page whose answer launches the same request again, so
    /// the checks run in one place, in this order, every time.
    pub fn open(
        &self,
        extension: &Extension,
        command: &ExtensionCommand,
        request: &LaunchRequest,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Opened> {
        let services = &self.state.services;
        let requested = RequestedCapabilities::read(extension.root())?;
        let approved = match services.permissions.state(extension.id(), &requested)? {
            PermissionState::Decided(approved) => approved,
            PermissionState::Ask(question) => {
                let page = PermissionPage::new(
                    extension,
                    requested,
                    question,
                    services.clone(),
                    request.clone(),
                );
                return Ok(Opened::Page(pages::handle(cx.new(|_| page))));
            }
        };

        if !services.preferences.missing(extension, command)?.is_empty() {
            let page = PreferencesPage::new(
                format!("{} Preferences", command.title()),
                scopes(extension, command)
                    .into_iter()
                    .map(|(scope, declarations)| (scope, declarations.to_vec()))
                    .collect(),
                services.clone(),
                Some(request.clone()),
            );
            return Ok(Opened::Page(pages::handle(cx.new(|_| page))));
        }

        let lacks_argument = command.arguments().iter().any(|argument| {
            argument.required
                && request
                    .arguments()
                    .get(argument.name.as_str())
                    .is_none_or(|value| value.trim().is_empty())
        });
        if lacks_argument {
            let page = ArgumentsPage::new(command, request.clone(), services.clone());
            return Ok(Opened::Page(pages::handle(cx.new(|_| page))));
        }

        let id = extension.id().to_string();
        let context = LaunchContext {
            command: command.id().clone(),
            arguments: request.arguments().clone(),
            preferences: services.preferences.resolve(extension, command)?,
            data_dir: services.data.extension_data_dir(&id),
            cache_dir: services.data.cache_dir(&id),
            development: services
                .development
                .borrow()
                .iter()
                .any(|directory| extension.root().starts_with(directory)),
        };
        let grant = requested.grant(&approved, extension.root(), context.data_dir());
        self.run(extension, command, context, approved, grant, window, cx)
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        extension: &Extension,
        command: &ExtensionCommand,
        context: LaunchContext,
        approved: BTreeSet<String>,
        grant: gpui_shell::Capabilities,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Opened> {
        let state = &self.state;
        let title = command.title().clone();
        if command.mode() == CommandMode::View
            && let Some(launch) = state.reusable(&context, &approved)
        {
            let view = state.launches.borrow()[&launch].view.clone();
            let page = cx.new(|cx| ScriptPage::new(title, view, cx));
            state.track_page(launch, &page, cx);
            return Ok(Opened::Page(pages::handle(page)));
        }
        state.release_idle(extension.id());

        std::fs::create_dir_all(context.cache_dir())
            .with_context(|| format!("cannot create {}", context.cache_dir().display()))?;
        let launch = state.lifecycle.borrow_mut().start();
        let sink = LaunchSink {
            effects: state.services.effects.clone(),
            state: Rc::downgrade(state),
            launch,
        };
        let policy = host_modules(&context, &sink).into_iter().try_fold(
            Policy::new()
                .with_application(extension.id())
                .with_capabilities(grant)
                .with_storage_path(state.services.data.storage_path(extension.id())),
            |policy, module| {
                policy
                    .with_host_module(module)
                    .map_err(|error| anyhow!("{error}"))
            },
        );
        let mounted = policy.and_then(|policy| {
            let policy = Rc::new(policy);
            let application = self
                .runtime
                .load_application_with_policy(
                    extension.root(),
                    command.module(),
                    policy.clone(),
                    window,
                    cx,
                )
                .with_context(|| format!("cannot load `{}`", command.id()))?;
            let view = self
                .runtime
                .mount_application(&application, window, cx)
                .with_context(|| format!("cannot start `{}`", command.id()))?;
            Ok((policy, view))
        });
        let (policy, view) = match mounted {
            Ok(mounted) => mounted,
            Err(error) => {
                state.lifecycle.borrow_mut().remove(launch);
                return Err(error);
            }
        };
        state.launches.borrow_mut().insert(
            launch,
            Launch {
                context,
                grant: approved,
                mode: command.mode(),
                policy,
                view: view.clone(),
            },
        );

        match command.mode() {
            CommandMode::View => {
                let page = cx.new(|cx| ScriptPage::new(title, view, cx));
                state.track_page(launch, &page, cx);
                Ok(Opened::Page(pages::handle(page)))
            }
            CommandMode::NoView => {
                // The command may already have finished during `init`; its
                // finish is deferred, so the launch is still here to bound.
                state.lifecycle.borrow_mut().acquire(launch);
                let weak = Rc::downgrade(state);
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(BACKGROUND_LIMIT).await;
                    cx.update(|cx| {
                        if let Some(state) = weak.upgrade() {
                            state.finish_background(launch, cx);
                        }
                    });
                })
                .detach();
                Ok(Opened::Background)
            }
        }
    }

    /// The installed extensions, with Update, Uninstall, Open Preferences and
    /// Reveal in File Manager, and "Install from Git…".
    pub fn extensions_page(&self, _: &mut Window, cx: &mut App) -> Result<PageHandle> {
        let page = ExtensionsPage::new(self.state.services.clone())?;
        Ok(pages::handle(cx.new(|_| page)))
    }

    /// Edits the preferences of an extension, and of one of its commands when
    /// `command` is given; with no command, of every command it has.
    pub fn preferences_page(
        &self,
        catalog: &Catalog,
        extension: &Extension,
        command: Option<&ExtensionCommand>,
        _: &mut Window,
        cx: &mut App,
    ) -> Result<PageHandle> {
        let sections = match command {
            Some(command) => scopes(extension, command)
                .into_iter()
                .map(|(scope, declarations)| (scope, declarations.to_vec()))
                .collect(),
            None => all_scopes(catalog, extension.id()),
        };
        let title = format!(
            "{} Preferences",
            command.map_or(extension.name(), |command| command.title())
        );
        let page = PreferencesPage::new(title, sections, self.state.services.clone(), None);
        Ok(pages::handle(cx.new(|_| page)))
    }
}

/// Builds the page an extension pushes: `callback` returns an instance of a
/// `View` subclass, which becomes a page of its own.
///
/// The new view runs under the policy of the view that registered the
/// callback, so a pushed page has the same grant and the same `launch()` as
/// the command that pushed it, and it keeps that launch loaded while it is on
/// the stack.
pub fn page_from_callback(
    callback: &ComponentCallback,
    title: SharedString,
    window: &mut Window,
    cx: &mut App,
) -> Result<PageHandle> {
    let view = callback.invoke_view(window, cx)?;
    let policy = view.read(cx).policy();
    let page = cx.new(|cx| ScriptPage::new(title, view, cx));
    let state = cx
        .try_global::<HostRegistry>()
        .and_then(|registry| registry.0.upgrade());
    if let Some(state) = state
        && let Some(launch) = state.launch_for(&policy)
    {
        state.track_page(launch, &page, cx);
    }
    Ok(pages::handle(page))
}

/// The user's language, as the POSIX locale variables or the platform say.
fn locale() -> SharedString {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty() && value != "C" && value != "POSIX")
        .map(|value| {
            value
                .split(['.', '@'])
                .next()
                .unwrap_or_default()
                .replace('_', "-")
        })
        .unwrap_or_else(|| "en".to_owned())
        .into()
}
