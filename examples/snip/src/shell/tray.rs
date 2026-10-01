//! The tray icon: Snip's only permanent presence on screen.
//!
//! `tray-icon` provides it on Windows and macOS. On Linux it would need
//! GTK, so Snip speaks the StatusNotifierItem D-Bus protocol directly
//! through `ksni`; desktops without a tray host (GNOME without its
//! AppIndicator extension) show nothing, and `snip settings` remains.
//!
//! Menu choices arrive on a platform thread and are handed to the main
//! thread through a channel, like hotkeys and other `snip` processes.

/// What the tray asks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayCommand {
    Capture,
    PinClipboard,
    CloseAllPins,
    Settings,
    Quit,
}

impl TrayCommand {
    const MENU: [Option<Self>; 7] = [
        Some(Self::Capture),
        Some(Self::PinClipboard),
        Some(Self::CloseAllPins),
        None,
        Some(Self::Settings),
        None,
        Some(Self::Quit),
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Capture => "Capture",
            Self::PinClipboard => "Pin clipboard",
            Self::CloseAllPins => "Close all pins",
            Self::Settings => "Settings…",
            Self::Quit => "Quit Snip",
        }
    }
}

/// The tray icon image: a crop mark on the brand color, drawn rather than
/// shipped so the example needs no binary assets.
fn icon_rgba(side: u32) -> Vec<u8> {
    use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
    let mut pixmap = Pixmap::new(side, side).expect("a non-empty icon");
    let side_f = side as f32;
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(0x1C, 0x7E, 0xD6, 0xFF);
    if let Some(path) = rounded_rect(side_f, side_f * 0.22) {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    paint.set_color_rgba8(0xFF, 0xFF, 0xFF, 0xFF);
    let stroke = Stroke {
        width: side_f * 0.09,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Stroke::default()
    };
    let (low, high) = (side_f * 0.3, side_f * 0.7);
    let mut marks = PathBuilder::new();
    // Two interlocking corners, as on a crop tool.
    marks.move_to(low, side_f * 0.2);
    marks.line_to(low, high);
    marks.line_to(side_f * 0.8, high);
    marks.move_to(side_f * 0.2, low);
    marks.line_to(high, low);
    marks.line_to(high, side_f * 0.8);
    if let Some(path) = marks.finish() {
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    // Straight alpha for the platforms.
    let mut pixels = pixmap.take();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    pixels
}

fn rounded_rect(side: f32, radius: f32) -> Option<tiny_skia::Path> {
    let mut path = tiny_skia::PathBuilder::new();
    path.move_to(radius, 0.);
    path.line_to(side - radius, 0.);
    path.quad_to(side, 0., side, radius);
    path.line_to(side, side - radius);
    path.quad_to(side, side, side - radius, side);
    path.line_to(radius, side);
    path.quad_to(0., side, 0., side - radius);
    path.line_to(0., radius);
    path.quad_to(0., 0., radius, 0.);
    path.close();
    path.finish()
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use native::{Tray, start};

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native {
    use anyhow::{Context as _, Result};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };

    use super::TrayCommand;

    /// Keeps the icon in the tray while alive.
    pub struct Tray {
        _icon: TrayIcon,
    }

    /// Shows the tray icon; `on_command` runs on a platform thread.
    pub fn start(on_command: impl Fn(TrayCommand) + Send + Sync + Clone + 'static) -> Result<Tray> {
        let menu = Menu::new();
        let mut items = Vec::new();
        for entry in TrayCommand::MENU {
            match entry {
                Some(command) => {
                    let item = MenuItem::new(command.title(), true, None);
                    menu.append(&item)?;
                    items.push((item.id().clone(), command));
                }
                None => menu.append(&PredefinedMenuItem::separator())?,
            }
        }
        let on_menu = on_command.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, command)) = items.iter().find(|(id, _)| *id == event.id) {
                on_menu(*command);
            }
        }));
        // A left click captures, as in most capture tools; the menu is on
        // the right button.
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                on_command(TrayCommand::Capture);
            }
        }));
        let side = 32;
        let icon = Icon::from_rgba(super::icon_rgba(side), side, side)
            .context("cannot build the tray icon")?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Snip")
            .with_icon(icon)
            .build()
            .context("cannot show the tray icon")?;
        Ok(Tray { _icon: icon })
    }
}

#[cfg(target_os = "linux")]
pub use status_notifier::{Tray, start};

#[cfg(target_os = "linux")]
mod status_notifier {
    use std::sync::Arc;

    use anyhow::{Context as _, Result};
    use ksni::{MenuItem, blocking::TrayMethods as _, menu::StandardItem};

    use super::TrayCommand;

    type Callback = Arc<dyn Fn(TrayCommand) + Send + Sync>;

    struct SnipTray {
        on_command: Callback,
    }

    impl ksni::Tray for SnipTray {
        fn id(&self) -> String {
            "gpui-kit-snip".into()
        }

        fn title(&self) -> String {
            "Snip".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let side = 32;
            // StatusNotifierItem wants ARGB in network byte order.
            let data = super::icon_rgba(side as u32)
                .chunks_exact(4)
                .flat_map(|pixel| [pixel[3], pixel[0], pixel[1], pixel[2]])
                .collect();
            vec![ksni::Icon {
                width: side,
                height: side,
                data,
            }]
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            (self.on_command)(TrayCommand::Capture);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            TrayCommand::MENU
                .into_iter()
                .map(|entry| match entry {
                    Some(command) => StandardItem {
                        label: command.title().into(),
                        activate: Box::new(move |tray: &mut Self| (tray.on_command)(command)),
                        ..Default::default()
                    }
                    .into(),
                    None => MenuItem::Separator,
                })
                .collect()
        }
    }

    /// Keeps the icon in the tray while alive.
    pub struct Tray {
        _handle: ksni::blocking::Handle<SnipTray>,
    }

    pub fn start(on_command: impl Fn(TrayCommand) + Send + Sync + Clone + 'static) -> Result<Tray> {
        let handle = SnipTray {
            on_command: Arc::new(on_command),
        }
        .spawn()
        .context("no tray host is available")?;
        Ok(Tray { _handle: handle })
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub struct Tray;

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn start(_: impl Fn(TrayCommand) + Send + Sync + Clone + 'static) -> anyhow::Result<Tray> {
    anyhow::bail!("this platform has no tray")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icon_is_opaque_inside_and_clear_at_corners() {
        let side = 32;
        let pixels = icon_rgba(side);
        assert_eq!(pixels.len(), (side * side * 4) as usize);
        let alpha = |x: u32, y: u32| pixels[((y * side + x) * 4 + 3) as usize];
        assert_eq!(alpha(16, 16), 255);
        assert_eq!(alpha(0, 0), 0, "rounded corner");
    }
}
