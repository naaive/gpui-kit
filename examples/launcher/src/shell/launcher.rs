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
    extensions::{Catalog, ExtensionHost, LaunchRequest},
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
/// installed copy of the same extension.
pub fn extension_roots(
    development: &[PathBuf],
    environment: Option<PathBuf>,
    configured: Option<&Path>,
    bundled: &Path,
) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let candidates = development
        .iter()
        .cloned()
        .chain(environment)
        .chain(configured.map(Path::to_path_buf))
        .chain([bundled.to_path_buf()]);
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
    hotkey: SummonHotkey,
    _tasks: Vec<Task<()>>,
}

impl Global for Launcher {}

impl Launcher {
    fn roots(&self) -> Vec<PathBuf> {
        extension_roots(
            &self.development_directories,
            std::env::var_os("LAUNCHER_EXTENSIONS").map(PathBuf::from),
            self.settings.extension_directory(),
            &self.bundled_extensions,
        )
    }
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
                    if cx.global::<Launcher>().hotkey.is_summon(id) {
                        toggle(cx);
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
        &startup.bundled_extensions,
    ));
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
        hotkey,
        _tasks: tasks,
    });

    apply_appearance(appearance, None, cx);
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
    if previous.appearance() != settings.appearance() {
        apply_appearance(settings.appearance(), Some(window), cx);
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

/// Runs `update` with the launcher window and its view, if one is open.
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
    with_window(cx, |window, view, cx| {
        window.activate_window();
        view.update(cx, |view, cx| view.reset(window, cx));
    });
}

fn hide_now(cx: &mut App) {
    if !cx
        .try_global::<Launcher>()
        .is_some_and(|launcher| launcher.is_visible)
    {
        return;
    }
    let launcher = cx.global_mut::<Launcher>();
    launcher.is_visible = false;
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
            extension_roots(&[], None, None, &bundled),
            [PathBuf::from("/bundled")]
        );
        assert_eq!(
            extension_roots(
                &["/dev/a".into(), "/dev/b".into()],
                Some("/env".into()),
                Some(Path::new("/configured")),
                &bundled,
            ),
            [
                PathBuf::from("/dev/a"),
                "/dev/b".into(),
                "/env".into(),
                "/configured".into(),
                "/bundled".into(),
            ]
        );
        assert_eq!(
            extension_roots(
                &["/same".into()],
                Some("/same".into()),
                Some(Path::new("/bundled")),
                &bundled
            ),
            [PathBuf::from("/same"), "/bundled".into()],
            "a directory is searched once, at its first position"
        );
    }
}
