//! The application: settings, the capturer, the open session, and the
//! inputs that start things — system-wide shortcuts, the tray and other
//! `snip` processes.

use std::{path::PathBuf, sync::Arc};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, AppContext as _, Entity, Global, Task, Window, actions};

use crate::{
    capture::{self, Capturer},
    output::clipboard::{GlobalClipboard, ImageClipboard},
    pin,
    scene::Style,
    session::{self, CaptureSession},
    settings_window,
    shell::{
        self,
        hotkey::{HotkeyCommand, HotkeyStatus, Hotkeys},
        ipc::{Listener, Message},
        settings::{Appearance, Settings},
        tray::{self, TrayCommand},
    },
};

actions!(
    snip,
    [
        /// Freezes the screen and starts selecting.
        Capture,
        /// Pins the clipboard's image or text.
        PinClipboard,
        CloseAllPins,
        OpenSettings,
        Quit,
    ]
);

/// Snip's application-wide state.
pub struct Snip {
    settings: Settings,
    settings_path: Option<PathBuf>,
    capturer: Arc<dyn Capturer>,
    hotkeys: Option<Hotkeys>,
    session: Option<Entity<CaptureSession>>,
    /// The capture being taken, before its session opens.
    capturing: Option<Task<()>>,
    /// Windows open hidden, for `--render-preview`.
    is_offscreen: bool,
    _tray: Option<tray::Tray>,
    _inputs: Vec<Task<()>>,
}

impl Global for Snip {}

/// What [`start`] needs from `main`.
pub struct Startup {
    capturer: Arc<dyn Capturer>,
    clipboard: Arc<dyn ImageClipboard>,
    settings_path: Option<PathBuf>,
    listener: Option<Listener>,
    with_system_integration: bool,
    is_offscreen: bool,
}

impl Startup {
    /// A real application: the platform capturer and clipboard, settings in
    /// the data directory, hotkeys and a tray icon.
    pub fn new(listener: Option<Listener>) -> Self {
        Self {
            capturer: capture::platform_capturer(),
            clipboard: Arc::new(crate::output::clipboard::SystemClipboard::default()),
            settings_path: shell::data_directory().map(|directory| directory.join("settings.json")),
            listener,
            with_system_integration: true,
            is_offscreen: false,
        }
    }

    /// The platform capturer, but a clipboard of its own, default settings
    /// and hidden windows: what `--render-preview` runs with.
    #[cfg(feature = "preview")]
    pub fn offscreen() -> Self {
        Self {
            capturer: capture::platform_capturer(),
            clipboard: Arc::new(crate::output::clipboard::MemoryClipboard::default()),
            settings_path: None,
            listener: None,
            with_system_integration: false,
            is_offscreen: true,
        }
    }

    /// No hotkeys, tray or socket, with the given capturer and clipboard.
    #[cfg(test)]
    pub fn isolated(
        capturer: Arc<dyn Capturer>,
        clipboard: Arc<dyn ImageClipboard>,
        settings_path: Option<PathBuf>,
    ) -> Self {
        Self {
            capturer,
            clipboard,
            settings_path,
            listener: None,
            with_system_integration: false,
            is_offscreen: true,
        }
    }
}

pub fn init(cx: &mut App) {
    session::init(cx);
    pin::init(cx);
    settings_window::init(cx);
    cx.on_action(|_: &Capture, cx| session::start(cx));
    cx.on_action(|_: &PinClipboard, cx| pin::pin_clipboard(cx));
    cx.on_action(|_: &CloseAllPins, cx| pin::close_all(cx));
    cx.on_action(|_: &OpenSettings, cx| settings_window::open(cx));
    cx.on_action(|_: &Quit, cx| cx.quit());
}

pub fn start(startup: Startup, cx: &mut App) {
    let settings = match &startup.settings_path {
        Some(path) => Settings::load(path).unwrap_or_else(|error| {
            tracing::error!("{error:#}; using the default settings");
            Settings::default()
        }),
        None => Settings::default(),
    };
    cx.set_global(GlobalClipboard(startup.clipboard));
    cx.set_global(Snip {
        settings,
        settings_path: startup.settings_path,
        capturer: startup.capturer,
        hotkeys: None,
        session: None,
        capturing: None,
        is_offscreen: startup.is_offscreen,
        _tray: None,
        _inputs: Vec::new(),
    });
    apply_appearance(None, cx);
    crate::raster::warm_up();
    if !startup.with_system_integration {
        return;
    }

    // Hotkeys, the tray and other processes all report on their own
    // threads; their requests are handled here, on the main thread.
    let (hotkey_sender, hotkey_receiver) = smol::channel::unbounded::<u32>();
    let mut hotkeys = Hotkeys::new(move |id| {
        hotkey_sender.try_send(id).ok();
    });
    let settings = &cx.global::<Snip>().settings;
    hotkeys.register(HotkeyCommand::Capture, settings.capture_shortcut());
    hotkeys.register(HotkeyCommand::PinClipboard, settings.pin_shortcut());

    let (tray_sender, tray_receiver) = smol::channel::unbounded::<TrayCommand>();
    let tray = tray::start(move |command| {
        tray_sender.try_send(command).ok();
    })
    .inspect_err(|error| tracing::warn!("{error:#}"))
    .ok();

    let (message_sender, message_receiver) = smol::channel::unbounded::<Message>();
    if let Some(listener) = startup.listener {
        shell::ipc::serve(listener, move |message| {
            message_sender.try_send(message).ok();
        });
    }

    let inputs = vec![
        cx.spawn(async move |cx| {
            while let Ok(id) = hotkey_receiver.recv().await {
                cx.update(|cx| {
                    let command = cx
                        .global::<Snip>()
                        .hotkeys
                        .as_ref()
                        .and_then(|hotkeys| hotkeys.command(id));
                    match command {
                        Some(HotkeyCommand::Capture) => session::start(cx),
                        Some(HotkeyCommand::PinClipboard) => pin::pin_clipboard(cx),
                        None => {}
                    }
                });
            }
        }),
        cx.spawn(async move |cx| {
            while let Ok(command) = tray_receiver.recv().await {
                cx.update(|cx| match command {
                    TrayCommand::Capture => session::start(cx),
                    TrayCommand::PinClipboard => pin::pin_clipboard(cx),
                    TrayCommand::CloseAllPins => pin::close_all(cx),
                    TrayCommand::Settings => settings_window::open(cx),
                    TrayCommand::Quit => cx.quit(),
                });
            }
        }),
        cx.spawn(async move |cx| {
            while let Ok(message) = message_receiver.recv().await {
                cx.update(|cx| handle(message, cx));
            }
        }),
    ];
    shell::platform::hide_dock_icon();
    let snip = cx.global_mut::<Snip>();
    snip.hotkeys = Some(hotkeys);
    snip._tray = tray;
    snip._inputs = inputs;
}

/// Does what another `snip` process asked.
pub fn handle(message: Message, cx: &mut App) {
    match message {
        Message::Start => {}
        Message::Capture => session::start(cx),
        Message::PinClipboard => pin::pin_clipboard(cx),
        Message::Settings => settings_window::open(cx),
        Message::Quit => cx.quit(),
    }
}

pub fn settings(cx: &App) -> &Settings {
    &cx.global::<Snip>().settings
}

/// Whether windows open hidden, rendered only to images.
pub fn is_offscreen(cx: &App) -> bool {
    cx.global::<Snip>().is_offscreen
}

pub fn capturer(cx: &App) -> Arc<dyn Capturer> {
    cx.global::<Snip>().capturer.clone()
}

pub fn hotkey_status(command: HotkeyCommand, cx: &App) -> HotkeyStatus {
    cx.global::<Snip>()
        .hotkeys
        .as_ref()
        .map_or(HotkeyStatus::Unavailable, |hotkeys| hotkeys.status(command))
}

/// Saves `settings` and applies what changed: shortcuts and appearance.
pub fn update_settings(settings: Settings, window: Option<&mut Window>, cx: &mut App) {
    let snip = cx.global_mut::<Snip>();
    let previous = std::mem::replace(&mut snip.settings, settings.clone());
    if let Some(hotkeys) = &mut snip.hotkeys {
        if previous.capture_shortcut() != settings.capture_shortcut() {
            hotkeys.register(HotkeyCommand::Capture, settings.capture_shortcut());
        }
        if previous.pin_shortcut() != settings.pin_shortcut() {
            hotkeys.register(HotkeyCommand::PinClipboard, settings.pin_shortcut());
        }
    }
    if let Some(path) = snip.settings_path.clone() {
        cx.background_spawn(async move {
            if let Err(error) = settings.save(&path) {
                tracing::error!("{error:#}");
            }
        })
        .detach();
    }
    if previous.appearance() != cx.global::<Snip>().settings.appearance() {
        apply_appearance(window, cx);
    }
}

/// Keeps the annotation style of a finished session for the next one.
pub fn remember_style(style: &Style, cx: &mut App) {
    let settings = settings(cx).clone().with_annotation_style(style);
    if &settings != self::settings(cx) {
        update_settings(settings, None, cx);
    }
}

pub fn apply_appearance(window: Option<&mut Window>, cx: &mut App) {
    match settings(cx).appearance() {
        Appearance::System => Theme::sync_system_appearance(window, cx),
        Appearance::Light => Theme::change(ThemeMode::Light, window, cx),
        Appearance::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
}

/// Whether a capture can start: none is open or being taken.
pub fn is_idle(cx: &App) -> bool {
    let snip = cx.global::<Snip>();
    snip.session.is_none() && snip.capturing.is_none()
}

pub fn capture_started(task: Task<()>, cx: &mut App) {
    cx.global_mut::<Snip>().capturing = Some(task);
}

/// The capture task is done; it is finishing its last step, so it is let
/// run to the end rather than dropped.
pub fn capture_finished(cx: &mut App) {
    if let Some(task) = cx.global_mut::<Snip>().capturing.take() {
        task.detach();
    }
}

pub fn begin_session(session: Entity<CaptureSession>, cx: &mut App) {
    cx.global_mut::<Snip>().session = Some(session);
}

pub fn end_session(cx: &mut App) {
    cx.global_mut::<Snip>().session = None;
}

pub fn session(cx: &App) -> Option<Entity<CaptureSession>> {
    cx.global::<Snip>().session.clone()
}
