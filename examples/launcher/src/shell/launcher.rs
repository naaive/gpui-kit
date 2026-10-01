//! The running launcher: its one window, how it is summoned and dismissed,
//! and the settings that shape it.
//!
//! Everything here runs on the GPUI main thread. Requests arriving from other
//! threads (the IPC listener, the global hotkey, deep links) are forwarded
//! through channels and handled by tasks this global owns.
//!
//! Showing and hiding always happen in a deferred callback: a request may come
//! from inside the launcher window's own update (an action, a form
//! submission), where the window cannot be updated or closed again.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context as _, Result};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Entity, Global, Subscription, Task,
    Window, WindowBounds, WindowKind, WindowOptions,
    component::{
        Theme, ThemeMode, WindowExt as _,
        notification::{Notification, NotificationType},
    },
    px, size,
};

use super::{
    deeplink,
    hotkey::{HotkeyStatus, SummonHotkey},
    ipc::{self, Listener, Message},
    settings::{Appearance, Settings},
};
use crate::{
    extensions::{Catalog, CommandId, ExtensionHost, LaunchRequest},
    model::Effect,
    ui::LauncherWindow,
};

/// How the launcher starts; built by `main` from the command line.
pub struct Startup {
    extensions: Rc<ExtensionHost>,
    bundled_extensions: PathBuf,
    development_directories: Vec<PathBuf>,
    listener: Option<Listener>,
    open_urls: Option<smol::channel::Receiver<String>>,
}

impl Startup {
    pub fn new(extensions: Rc<ExtensionHost>, bundled_extensions: PathBuf) -> Self {
        Self {
            extensions,
            bundled_extensions,
            development_directories: Vec::new(),
            listener: None,
            open_urls: None,
        }
    }

    pub fn with_development_directory(mut self, directory: PathBuf) -> Self {
        self.development_directories.push(directory);
        self
    }

    /// The socket other `launcher` processes send their requests to.
    pub fn with_listener(mut self, listener: Listener) -> Self {
        self.listener = Some(listener);
        self
    }

    /// Deep links the platform asks the application to open.
    pub fn with_open_urls(mut self, urls: smol::channel::Receiver<String>) -> Self {
        self.open_urls = Some(urls);
        self
    }
}

/// The directories extensions are discovered in, in order; an earlier one
/// wins a duplicate extension id. A development directory overrides an
/// installed copy of the same extension, and an installed copy overrides the
/// bundled one.
pub fn extension_roots(
    development: &[PathBuf],
    environment: Option<PathBuf>,
    configured: Option<&Path>,
    installed: &Path,
    bundled: &Path,
) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let candidates = development
        .iter()
        .cloned()
        .chain(environment)
        .chain(configured.map(Path::to_path_buf))
        .chain([installed.to_path_buf(), bundled.to_path_buf()]);
    for root in candidates {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

struct OpenWindow {
    handle: AnyWindowHandle,
    view: Entity<LauncherWindow>,
    _subscriptions: Vec<Subscription>,
}

pub struct Launcher {
    extensions: Rc<ExtensionHost>,
    bundled_extensions: PathBuf,
    development_directories: Vec<PathBuf>,
    settings: Settings,
    settings_path: Option<PathBuf>,
    catalog: Rc<Catalog>,
    /// The catalog no longer matches the extension directories; it is rebuilt
    /// the next time the launcher is shown, not while the user is using it.
    catalog_is_stale: bool,
    window: Option<OpenWindow>,
    is_visible: bool,
    /// What the window showed when it last hid, and when, to come back to.
    left: Option<(std::time::Instant, crate::ui::Snapshot)>,
    hotkey: SummonHotkey,
    _tasks: Vec<Task<()>>,
}

impl Global for Launcher {}

impl Launcher {
    /// Directories whose extensions count as in development: the ones named
    /// by `launcher dev` and `LAUNCHER_EXTENSIONS`.
    fn development_roots(&self) -> Vec<PathBuf> {
        self.development_directories
            .iter()
            .cloned()
            .chain(std::env::var_os("LAUNCHER_EXTENSIONS").map(PathBuf::from))
            .collect()
    }

    fn roots(&self) -> Vec<PathBuf> {
        extension_roots(
            &self.development_directories,
            std::env::var_os("LAUNCHER_EXTENSIONS").map(PathBuf::from),
            self.settings.extension_directory(),
            &self.extensions.data().extensions_dir(),
            &self.bundled_extensions,
        )
    }
}

/// Builds the page listing installed extensions, where they are installed
/// from Git, updated and removed.
pub fn extensions_page(
    window: &mut Window,
    cx: &mut App,
) -> anyhow::Result<crate::pages::PageHandle> {
    let host = cx
        .try_global::<Launcher>()
        .map(|launcher| launcher.extensions.clone())
        .ok_or_else(|| anyhow::anyhow!("the launcher is not running"))?;
    host.extensions_page(window, cx)
}

/// Builds the preferences page of a command's extension, with the command's
/// own settings after the extension's.
pub fn preferences_page(
    command: &CommandId,
    window: &mut Window,
    cx: &mut App,
) -> anyhow::Result<crate::pages::PageHandle> {
    let (host, catalog) = cx
        .try_global::<Launcher>()
        .map(|launcher| (launcher.extensions.clone(), launcher.catalog.clone()))
        .ok_or_else(|| anyhow::anyhow!("the launcher is not running"))?;
    let (extension, command) = catalog
        .command(command)
        .ok_or_else(|| anyhow::anyhow!("no command `{command}`"))?;
    host.preferences_page(&catalog, extension, Some(command), window, cx)
}

/// Starts the launcher: loads settings, applies them, listens for requests
/// and shows the window.
pub fn start(startup: Startup, cx: &mut App) {
    let settings_path = super::settings::settings_path();
    let settings = settings_path
        .as_deref()
        .map(|path| {
            Settings::load(path).unwrap_or_else(|error| {
                tracing::error!("{error:#}; using the default settings");
                Settings::default()
            })
        })
        .unwrap_or_default();

    let (requests, incoming) = smol::channel::unbounded::<Message>();
    let (presses, pressed) = smol::channel::unbounded::<u32>();
    let mut hotkey = SummonHotkey::new(move |id| {
        presses.try_send(id).ok();
    });
    hotkey.register(settings.summon_shortcut());
    if let Some(listener) = startup.listener {
        ipc::serve(listener, move |message| {
            requests.send_blocking(message).ok();
        });
    }

    let mut tasks = vec![
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(message) = incoming.recv().await {
                cx.update(|cx| handle(message, cx));
            }
        }),
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(id) = pressed.recv().await {
                cx.update(|cx| {
                    let hotkey = &cx.global::<Launcher>().hotkey;
                    if hotkey.is_summon(id) {
                        toggle(cx);
                    } else if let Some(item) = hotkey.command(id).map(str::to_owned) {
                        open_item(item, cx);
                    }
                });
            }
        }),
    ];
    if let Some(urls) = startup.open_urls {
        tasks.push(cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(url) = urls.recv().await {
                cx.update(|cx| handle(Message::Open(url), cx));
            }
        }));
    }

    let catalog = Catalog::discover(&extension_roots(
        &startup.development_directories,
        std::env::var_os("LAUNCHER_EXTENSIONS").map(PathBuf::from),
        settings.extension_directory(),
        &startup.extensions.data().extensions_dir(),
        &startup.bundled_extensions,
    ));
    // Installing, updating or removing an extension changes the catalog; like
    // any other change, it is read again the next time the launcher is shown.
    startup.extensions.set_extensions_changed_handler(|cx| {
        if cx.has_global::<Launcher>() {
            cx.global_mut::<Launcher>().catalog_is_stale = true;
        }
    });
    let appearance = settings.appearance();
    cx.set_global(Launcher {
        extensions: startup.extensions,
        bundled_extensions: startup.bundled_extensions,
        development_directories: startup.development_directories,
        settings,
        settings_path,
        catalog: Rc::new(catalog),
        catalog_is_stale: false,
        window: None,
        is_visible: false,
        left: None,
        hotkey,
        _tasks: tasks,
    });

    let launcher = cx.global::<Launcher>();
    launcher
        .extensions
        .set_development_directories(launcher.development_roots());
    crate::themes::register(cx);
    let settings = &cx.global::<Launcher>().settings;
    let (light, dark) = (
        settings.theme(false).map(str::to_owned),
        settings.theme(true).map(str::to_owned),
    );
    crate::themes::apply(light.as_deref(), dark.as_deref(), cx);
    apply_appearance(appearance, None, cx);

    crate::clipboard::start(cx);
    crate::sources::currency::start();
    crate::quicklinks::start(cx);
    crate::snippets::start(cx);
    crate::customizations::start(cx);
    register_command_hotkeys(cx);
    let expands = cx.global::<Launcher>().settings.expands_snippets();
    crate::snippets::set_expansion(expands, cx);
    crate::hyper_key::set(cx.global::<Launcher>().settings.hyper_key());
    crate::calendar::start(cx);
    crate::focus::start(cx);
    crate::reminders::start(cx);
    super::platform::hide_dock_icon();
    show(cx);
}

/// Carries out a request from another process or a deep link.
pub fn handle(message: Message, cx: &mut App) {
    match message {
        Message::Toggle => toggle(cx),
        Message::Show => show(cx),
        Message::Hide => hide(cx),
        Message::Open(link) => match deeplink::parse(&link) {
            Ok(request) => open_command(request, cx),
            Err(error) => {
                tracing::warn!("{error:#}");
                show(cx);
                cx.defer(move |cx| {
                    with_window(cx, |window, _, cx| {
                        window.push_notification(
                            Notification::new()
                                .title("Couldn’t open the link")
                                .message(format!("{error:#}"))
                                .with_type(NotificationType::Error),
                            cx,
                        )
                    });
                });
            }
        },
        Message::Dev(directory) => add_development_directory(directory, cx),
    }
}

/// Shows the launcher, or hides it if it is showing.
pub fn toggle(cx: &mut App) {
    cx.defer(|cx| {
        if cx
            .try_global::<Launcher>()
            .is_some_and(|launcher| launcher.is_visible)
        {
            hide_now(cx);
        } else {
            show_now(cx);
        }
    });
}

/// Brings the launcher to the front on a fresh root search.
pub fn show(cx: &mut App) {
    cx.defer(show_now);
}

/// Hides the launcher and returns to the application that was in front.
///
/// The window stays alive where the platform can hide it (macOS); elsewhere
/// GPUI cannot hide a window, so it is closed and opened again next time.
pub fn hide(cx: &mut App) {
    cx.defer(hide_now);
}

/// Shows the launcher and opens a command, as a deep link asks.
pub fn open_command(request: LaunchRequest, cx: &mut App) {
    cx.defer(move |cx| {
        show_now(cx);
        with_window(cx, |window, view, cx| {
            view.update(cx, |view, cx| view.open_command(request, window, cx))
        });
    });
}

/// Loads an extension directory ahead of all others, as `launcher dev` asks.
pub fn add_development_directory(directory: PathBuf, cx: &mut App) {
    cx.defer(move |cx| {
        let launcher = cx.global_mut::<Launcher>();
        launcher.development_directories.retain(|d| *d != directory);
        launcher.development_directories.insert(0, directory);
        launcher.catalog_is_stale = true;
        launcher
            .extensions
            .set_development_directories(launcher.development_roots());
        // A window that is showing keeps the old catalog until shown again.
        hide_now(cx);
        show_now(cx);
    });
}

/// The saved settings, or the defaults where the launcher is not running
/// (tests that open a page on their own).
pub fn settings(cx: &App) -> Settings {
    cx.try_global::<Launcher>()
        .map(|launcher| launcher.settings.clone())
        .unwrap_or_default()
}

pub fn hotkey_status(cx: &App) -> Option<HotkeyStatus> {
    cx.try_global::<Launcher>()
        .map(|launcher| launcher.hotkey.status().clone())
}

/// Saves settings and applies them: the shortcut and the appearance at once,
/// the extensions folder the next time the launcher opens.
pub fn update_settings(settings: Settings, window: &mut Window, cx: &mut App) -> Result<()> {
    let launcher = cx
        .try_global::<Launcher>()
        .context("the launcher is not running")?;
    let path = launcher
        .settings_path
        .clone()
        .context("this system has no data directory for settings")?;
    settings.save(&path)?;

    let previous = std::mem::replace(&mut cx.global_mut::<Launcher>().settings, settings.clone());
    let launcher = cx.global_mut::<Launcher>();
    if previous.summon_shortcut() != settings.summon_shortcut()
        || *launcher.hotkey.status() != HotkeyStatus::Registered
    {
        launcher.hotkey.register(settings.summon_shortcut());
    }
    if previous.extension_directory() != settings.extension_directory() {
        launcher.catalog_is_stale = true;
    }
    if previous.theme(false) != settings.theme(false)
        || previous.theme(true) != settings.theme(true)
    {
        crate::themes::apply(settings.theme(false), settings.theme(true), cx);
    }
    if previous.appearance() != settings.appearance() {
        apply_appearance(settings.appearance(), Some(window), cx);
    }
    if previous.expands_snippets() != settings.expands_snippets() {
        crate::snippets::set_expansion(settings.expands_snippets(), cx);
    }
    if previous.calendar_feeds() != settings.calendar_feeds() {
        let schedule = crate::calendar::schedule(cx);
        schedule.update(cx, |schedule, cx| schedule.refresh(true, cx));
    }
    if previous.hyper_key() != settings.hyper_key() {
        crate::hyper_key::set(settings.hyper_key());
    }
    Ok(())
}

fn apply_appearance(appearance: Appearance, window: Option<&mut Window>, cx: &mut App) {
    match appearance {
        Appearance::System => Theme::sync_system_appearance(window, cx),
        Appearance::Light => Theme::change(ThemeMode::Light, window, cx),
        Appearance::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
}

/// Reads settings and data again after they were replaced on disk, as an
/// import does, and applies them without a restart.
pub fn reload_data(cx: &mut App) {
    if !cx.has_global::<Launcher>() {
        return;
    }
    // The next summon builds a fresh window on the reloaded stores.
    hide_now(cx);
    close_window(cx);
    let old_hotkeys: Vec<String> = crate::customizations::store(cx)
        .map(|store| {
            store
                .read(cx)
                .hotkeys()
                .map(|(item, _)| item.to_owned())
                .collect()
        })
        .unwrap_or_default();
    for item in old_hotkeys {
        cx.global_mut::<Launcher>().hotkey.unregister_command(&item);
    }
    let settings = cx
        .global::<Launcher>()
        .settings_path
        .as_deref()
        .and_then(|path| Settings::load(path).ok())
        .unwrap_or_default();
    let previous = std::mem::replace(&mut cx.global_mut::<Launcher>().settings, settings.clone());
    if previous.summon_shortcut() != settings.summon_shortcut() {
        cx.global_mut::<Launcher>()
            .hotkey
            .register(settings.summon_shortcut());
    }
    if previous.extension_directory() != settings.extension_directory() {
        cx.global_mut::<Launcher>().catalog_is_stale = true;
    }
    crate::themes::apply(settings.theme(false), settings.theme(true), cx);
    apply_appearance(settings.appearance(), None, cx);
    crate::quicklinks::start(cx);
    crate::snippets::start(cx);
    crate::customizations::start(cx);
    register_command_hotkeys(cx);
    crate::snippets::set_expansion(settings.expands_snippets(), cx);
    crate::hyper_key::set(settings.hyper_key());
    crate::focus::reload(cx);
    crate::reminders::reload(cx);
    crate::notes::reload_open_note(cx);
    let schedule = crate::calendar::schedule(cx);
    schedule.update(cx, |schedule, cx| schedule.refresh(true, cx));
}

/// Runs `update` with the launcher window and its view, if one is open.
/// Registers the hotkeys saved for root search items.
fn register_command_hotkeys(cx: &mut App) {
    let Some(store) = crate::customizations::store(cx) else {
        return;
    };
    let saved: Vec<(String, String)> = store
        .read(cx)
        .hotkeys()
        .map(|(item, shortcut)| (item.to_owned(), shortcut.to_owned()))
        .collect();
    let hotkey = &mut cx.global_mut::<Launcher>().hotkey;
    for (item, shortcut) in saved {
        if let Err(error) = hotkey.register_command(&item, &shortcut) {
            tracing::warn!("cannot register the hotkey `{shortcut}` of {item}: {error:#}");
        }
    }
}

/// Sets the global hotkey that opens the root search item `item`, or
/// removes it when `shortcut` is empty, and saves the choice.
pub fn set_command_hotkey(item: &str, shortcut: &str, cx: &mut App) -> Result<()> {
    let shortcut = shortcut.trim();
    {
        let hotkey = &mut cx.global_mut::<Launcher>().hotkey;
        match shortcut.is_empty() {
            true => hotkey.unregister_command(item),
            false => hotkey.register_command(item, shortcut)?,
        }
    }
    if let Some(store) = crate::customizations::store(cx) {
        store.update(cx, |store, cx| store.set_hotkey(item, shortcut, cx));
    }
    Ok(())
}

/// Shows the launcher and opens the root search item `item`, as its hotkey
/// asks.
pub fn open_item(item: String, cx: &mut App) {
    cx.defer(move |cx| {
        show_now(cx);
        with_window(cx, |window, view, cx| {
            let found = view.update(cx, |view, cx| view.run_root_item(&item, window, cx));
            if !found {
                window.push_notification(
                    Notification::new()
                        .title("This command no longer exists")
                        .message("Remove its hotkey from the command’s actions.")
                        .with_type(NotificationType::Warning),
                    cx,
                );
            }
        });
    });
}

/// Performs `effect` in the launcher window once the current update is over,
/// for built-in pages whose callbacks run inside the window's own update.
pub fn perform(effect: Effect, cx: &mut App) {
    cx.defer(move |cx| {
        with_window(cx, |window, view, cx| {
            view.update(cx, |view, cx| view.perform_effect(effect, window, cx))
        })
    });
}

fn with_window(cx: &mut App, update: impl FnOnce(&mut Window, &Entity<LauncherWindow>, &mut App)) {
    let Some((handle, view)) = cx
        .try_global::<Launcher>()
        .and_then(|launcher| launcher.window.as_ref())
        .map(|open| (open.handle, open.view.clone()))
    else {
        return;
    };
    handle
        .update(cx, |_, window, cx| update(window, &view, cx))
        .ok();
}

fn show_now(cx: &mut App) {
    if !cx.has_global::<Launcher>() {
        return;
    }
    // Before the launcher's window opens and takes the front. The launcher's
    // own windows are ignored, so showing it again keeps the earlier one.
    crate::window_layout::remember_frontmost();
    crate::selection::capture();
    let launcher = cx.global_mut::<Launcher>();
    if launcher.catalog_is_stale {
        launcher.catalog = Rc::new(Catalog::discover(&launcher.roots()));
        launcher.catalog_is_stale = false;
        close_window(cx);
    }
    // The window may have been closed by the window manager.
    let windows = cx.windows();
    let is_open = cx
        .global::<Launcher>()
        .window
        .as_ref()
        .is_some_and(|open| windows.contains(&open.handle));
    if !is_open {
        cx.global_mut::<Launcher>().window = None;
        if let Err(error) = open_window(cx) {
            tracing::error!("cannot open the launcher window: {error:#}");
            return;
        }
    }
    cx.global_mut::<Launcher>().is_visible = true;
    cx.activate(true);
    let pop_to_root = settings(cx).pop_to_root();
    let left = cx
        .global_mut::<Launcher>()
        .left
        .take()
        .filter(|(at, _)| pop_to_root.keeps(at.elapsed()));
    with_window(cx, |window, view, cx| {
        window.activate_window();
        view.update(cx, |view, cx| match left {
            Some((_, snapshot)) => view.restore(snapshot, window, cx),
            None => view.reset(window, cx),
        });
    });
}

fn hide_now(cx: &mut App) {
    if !cx
        .try_global::<Launcher>()
        .is_some_and(|launcher| launcher.is_visible)
    {
        return;
    }
    let snapshot = cx
        .global::<Launcher>()
        .window
        .as_ref()
        .map(|open| open.view.read(cx).snapshot());
    let launcher = cx.global_mut::<Launcher>();
    launcher.is_visible = false;
    launcher.left = snapshot.map(|snapshot| (std::time::Instant::now(), snapshot));
    if cfg!(target_os = "macos") {
        cx.hide();
    } else {
        close_window(cx);
    }
}

fn close_window(cx: &mut App) {
    if let Some(open) = cx.global_mut::<Launcher>().window.take() {
        open.handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

fn open_window(cx: &mut App) -> Result<()> {
    let launcher = cx.global::<Launcher>();
    let (catalog, extensions) = (launcher.catalog.clone(), launcher.extensions.clone());
    let mut subscriptions = Vec::new();
    let (handle, view) = gpui_kit::open_window(window_options(cx), cx, |window, cx| {
        subscriptions.push(window.observe_window_appearance(|window, cx| {
            if settings(cx).appearance() == Appearance::System {
                Theme::sync_system_appearance(Some(window), cx);
            }
        }));
        cx.new(|cx| {
            // Clicking another application dismisses the launcher.
            subscriptions.push(cx.observe_window_activation(window, |_, window, cx| {
                if !window.is_window_active() {
                    hide(cx);
                }
            }));
            LauncherWindow::new(catalog, extensions, window, cx)
        })
    })?;
    cx.global_mut::<Launcher>().window = Some(OpenWindow {
        handle,
        view,
        _subscriptions: subscriptions,
    });
    Ok(())
}

/// A borderless window above other windows, as far as each platform allows.
///
/// macOS and Windows get a pop-up: a non-activating panel that joins every
/// space on macOS, a topmost tool window on Windows. On X11 a pop-up is
/// override-redirect and never receives keyboard focus, so Linux gets a
/// normal window with client-side decorations instead.
fn window_options(cx: &App) -> WindowOptions {
    let size = size(px(750.), px(475.));
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
        titlebar: None,
        focus: true,
        show: true,
        kind: if cfg!(any(target_os = "macos", target_os = "windows")) {
            WindowKind::PopUp
        } else {
            WindowKind::Normal
        },
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_decorations: cfg!(target_os = "linux")
            .then_some(gpui_kit::WindowDecorations::Client),
        app_id: Some("gpui-kit-launcher".into()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extension_roots_order_and_duplicates() {
        let bundled = PathBuf::from("/bundled");
        assert_eq!(
            extension_roots(&[], None, None, Path::new("/installed"), &bundled),
            [PathBuf::from("/installed"), "/bundled".into()]
        );
        assert_eq!(
            extension_roots(
                &["/dev/a".into(), "/dev/b".into()],
                Some("/env".into()),
                Some(Path::new("/configured")),
                Path::new("/installed"),
                &bundled,
            ),
            [
                PathBuf::from("/dev/a"),
                "/dev/b".into(),
                "/env".into(),
                "/configured".into(),
                "/installed".into(),
                "/bundled".into(),
            ]
        );
        assert_eq!(
            extension_roots(
                &["/same".into()],
                Some("/same".into()),
                Some(Path::new("/bundled")),
                Path::new("/same"),
                &bundled
            ),
            [PathBuf::from("/same"), "/bundled".into()],
            "a directory is searched once, at its first position"
        );
    }
}
