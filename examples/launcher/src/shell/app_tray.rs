//! The launcher's own icon in the system tray (the menu bar on macOS): a
//! click summons or dismisses the launcher, and its menu opens the launcher,
//! its settings, or quits it.
//!
//! Linux has no tray here; the setting does nothing there.

use gpui_kit::App;

/// Shows the tray icon or removes it, as the settings say.
pub fn sync(cx: &mut App) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    platform::sync(cx);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = cx;
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform {
    use std::{path::Path, time::Duration};

    use anyhow::Result;
    use gpui_kit::{App, AsyncApp, Global, Task};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuItem, PredefinedMenuItem},
    };

    use crate::{
        model::Image,
        shell::{launcher, tray},
    };

    /// The tray's id; its menu entries are `launcher#<entry>`, which the
    /// commands' trays never use, since a command id has a slash.
    const ID: &str = "launcher";
    const OPEN: &str = "launcher#open";
    const SETTINGS: &str = "launcher#settings";
    const QUIT: &str = "launcher#quit";

    /// How long after hiding a press on the icon still counts as made while
    /// the launcher showed: pressing the icon takes the focus from the
    /// launcher, which hides it just before the press arrives.
    const DISMISS_GRACE: Duration = Duration::from_millis(300);

    enum Event {
        Menu(String),
        Press,
        Release,
    }

    struct AppTray {
        icon: Option<TrayIcon>,
        /// Whether the launcher showed when the icon was pressed, so the
        /// release that follows hides it rather than showing it again.
        pressed_while_showing: bool,
        _events: Task<()>,
    }

    impl Global for AppTray {}

    pub fn sync(cx: &mut App) {
        if !cx.has_global::<AppTray>() {
            let events = listen(cx);
            cx.set_global(AppTray {
                icon: None,
                pressed_while_showing: false,
                _events: events,
            });
        }
        let wanted = launcher::settings(cx).shows_tray_icon();
        let app_tray = cx.global_mut::<AppTray>();
        match (wanted, app_tray.icon.is_some()) {
            (true, false) => match build() {
                Ok(icon) => app_tray.icon = Some(icon),
                Err(error) => tracing::warn!("cannot show the launcher in the tray: {error:#}"),
            },
            (false, true) => app_tray.icon = None,
            _ => {}
        }
    }

    /// Forwards the tray's clicks and menu choices to the main thread.
    fn listen(cx: &mut App) -> Task<()> {
        let (events, received) = smol::channel::unbounded::<Event>();
        let menu_events = events.clone();
        tray::subscribe_menu_events(move |id| {
            if id
                .strip_prefix(ID)
                .is_some_and(|entry| entry.starts_with('#'))
            {
                menu_events.try_send(Event::Menu(id.to_owned())).ok();
            }
        });
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                id,
                button: MouseButton::Left,
                button_state,
                ..
            } = event
                && id.0 == ID
            {
                let event = match button_state {
                    MouseButtonState::Down => Event::Press,
                    MouseButtonState::Up => Event::Release,
                };
                events.try_send(event).ok();
            }
        }));
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(event) = received.recv().await {
                cx.update(|cx| handle(event, cx));
            }
        })
    }

    fn handle(event: Event, cx: &mut App) {
        match event {
            Event::Menu(id) => match id.as_str() {
                OPEN => launcher::show(cx),
                SETTINGS => crate::settings_window::open(cx),
                QUIT => cx.quit(),
                _ => {}
            },
            Event::Press => {
                let showing = launcher::is_showing_or_hid_within(DISMISS_GRACE, cx);
                cx.global_mut::<AppTray>().pressed_while_showing = showing;
            }
            Event::Release => {
                match std::mem::take(&mut cx.global_mut::<AppTray>().pressed_while_showing) {
                    true => launcher::hide(cx),
                    false => launcher::show(cx),
                }
            }
        }
    }

    fn build() -> Result<TrayIcon> {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id(OPEN, "Open Launcher", true, None))?;
        menu.append(&MenuItem::with_id(SETTINGS, "Settings…", true, None))?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&MenuItem::with_id(QUIT, "Quit Launcher", true, None))?;
        let (pixels, width, height) = tray::icon_pixels(
            &Image::Icon("command".into()),
            Path::new("."),
            tray::icon_color(),
        )?;
        Ok(TrayIconBuilder::new()
            .with_id(ID)
            .with_menu(Box::new(menu))
            // A click summons the launcher; the menu is on the other button.
            .with_menu_on_left_click(false)
            .with_tooltip("GPUI Kit Launcher")
            .with_icon(Icon::from_rgba(pixels, width, height)?)
            .with_icon_as_template(cfg!(target_os = "macos"))
            .build()?)
    }
}
