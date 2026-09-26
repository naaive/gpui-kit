use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{App, AppContext as _, Entity, Global, SharedString, Window};
use gpui_shell::{
    ComponentCallback, HostError, HostModule, HostObject, HostValue, ScriptView, ShellRuntime,
    policy::Policy,
};

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
    model::{Effect, Toast, ToastStyle},
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

    /// Requests from inside a host function, where only the ambient `App` is
    /// at hand.
    fn request_from_script(&self, effect: Effect) {
        let sink = self.clone();
        if gpui_shell::with_current_app(|cx| sink.request(effect, cx)).is_none() {
            tracing::warn!("an extension requested an effect outside of a script call");
        }
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

/// What the launcher's own extension pages work with.
#[derive(Clone)]
pub(super) struct Services {
    pub(super) data: DataDirectory,
    pub(super) permissions: PermissionStore,
    pub(super) preferences: PreferenceStore,
    pub(super) effects: EffectSink,
    pub(super) changed: ChangeNotifier,
}

/// Everything a command is opened with, and what `launch()` answers.
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchContext {
    command: CommandId,
    arguments: BTreeMap<SharedString, SharedString>,
    preferences: ResolvedPreferences,
    data_dir: PathBuf,
    cache_dir: PathBuf,
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

    /// `launch()`'s return value.
    fn to_host_value(&self) -> HostValue {
        let arguments = self
            .arguments
            .iter()
            .fold(HostObject::new(), |object, (name, value)| {
                object.field(name.to_string(), value.to_string())
            });
        let preferences = self
            .preferences
            .iter()
            .fold(HostObject::new(), |object, (name, value)| {
                object.field(name.clone(), json_to_host(value))
            });
        HostObject::new()
            .field("extension", self.command.extension().to_string())
            .field("command", self.command.command().to_string())
            .field("arguments", arguments)
            .field("preferences", preferences)
            .into()
    }
}

fn json_to_host(value: &serde_json::Value) -> HostValue {
    match value {
        serde_json::Value::Null => HostValue::Null,
        serde_json::Value::Bool(value) => HostValue::Bool(*value),
        serde_json::Value::Number(value) => HostValue::Number(value.as_f64().unwrap_or_default()),
        serde_json::Value::String(value) => HostValue::Str(value.clone()),
        serde_json::Value::Array(values) => {
            HostValue::Array(values.iter().map(json_to_host).collect())
        }
        serde_json::Value::Object(fields) => HostValue::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), json_to_host(value)))
                .collect(),
        ),
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
    fn request(&self, effect: Effect) {
        self.effects.request_from_script(effect);
    }

    /// The command said it is done (a HUD, closing the window). Releasing its
    /// view waits until the script call that said so has returned.
    fn finished(&self) {
        let (state, launch) = (self.state.clone(), self.launch);
        gpui_shell::with_current_app(|cx| {
            cx.defer(move |cx| {
                if let Some(state) = state.upgrade() {
                    state.finish_background(launch, cx);
                }
            })
        });
    }
}

/// The HostModules a launch's policy grants: the only place that decides
/// what an extension can import besides the runtime's own modules.
///
/// When the bridge provides `HostApi::module_for(ExtensionContext)` and
/// `utils_module_source()`, this becomes
/// `vec![bridge::HostApi::module_for(context.into()), HostModule::source("launcher/utils", bridge::utils_module_source())]`,
/// with [`LaunchSink`] as the context's effect sink.
fn host_modules(context: &LaunchContext, sink: &LaunchSink) -> Vec<HostModule> {
    vec![api_module(context, sink)]
}

const API_MODULE: &str = "launcher/api";

const API_DECLARATIONS: &str = r#"
/** The command this view was opened for, with its arguments and preferences. */
export function launch(): {
  extension: string;
  command: string;
  arguments: Record<string, string>;
  preferences: Record<string, string | boolean>;
};
/** Shows a message in the launcher. */
export function show_toast(message: string, style?: "info" | "success" | "failure"): void;
/** Hides the launcher and shows a short message; a no-view command's way of finishing. */
export function show_hud(message: string): void;
/** Hides the launcher. */
export function close_main_window(): void;
"#;

/// `launcher/api` for one launch.
///
/// Built per launch, so `launch()` answers for the command whose code is
/// calling at any time — in `init`, in `render`, in a callback three seconds
/// later — rather than for whichever command was opened last.
fn api_module(context: &LaunchContext, sink: &LaunchSink) -> HostModule {
    let launch = context.to_host_value();
    let toast = sink.clone();
    let hud = sink.clone();
    let close = sink.clone();
    HostModule::new(API_MODULE)
        .function("launch", move |_| Ok(launch.clone()))
        .function("show_toast", move |arguments| {
            let message = arguments.string(0)?.to_owned();
            let style = match arguments.get(1) {
                None | Some(HostValue::Null) => ToastStyle::Info,
                Some(_) => match arguments.string(1)? {
                    "info" => ToastStyle::Info,
                    "success" => ToastStyle::Success,
                    "failure" => ToastStyle::Failure,
                    other => {
                        return Err(HostError::new(format!(
                            "unknown toast style `{other}`; use info, success or failure"
                        )));
                    }
                },
            };
            toast.request(Effect::ShowToast(Toast::new(style, message)));
            Ok(HostValue::Null)
        })
        .function("show_hud", move |arguments| {
            let message = arguments.string(0)?.to_owned();
            hud.request(Effect::ShowHud(message.into()));
            hud.finished();
            Ok(HostValue::Null)
        })
        .function("close_main_window", move |_| {
            close.request(Effect::CloseWindow);
            close.finished();
            Ok(HostValue::Null)
        })
        .declarations(API_DECLARATIONS)
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
