//! Icons in the system tray (the menu bar on macOS) for menu-bar commands.
//!
//! Each command gets one [`Tray`]. Menu entries are numbered per tray, and a
//! choice comes back through [`menu_events`] as the tray's id and the entry's
//! number, which the caller maps back to the entry's action.

use std::path::Path;

use anyhow::{Context as _, Result};
use gpui_kit::{AssetSource as _, SharedString};

use crate::model::{Image, MenuBarEntry, MenuBarModel};

/// The size icons are drawn at; the system scales them to the tray.
const ICON_SIZE: u32 = 32;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use platform::{Tray, icon_color, menu_events, subscribe_menu_events};

/// Draws an image as tray icon pixels: straight RGBA, `ICON_SIZE` square for
/// an SVG. A Lucide icon or an SVG is drawn in `color`, since a tray is not
/// the launcher's theme.
pub fn icon_pixels(image: &Image, root: &Path, color: &str) -> Result<(Vec<u8>, u32, u32)> {
    let svg = |data: Vec<u8>| -> Result<(Vec<u8>, u32, u32)> {
        let text = String::from_utf8(data)?.replace("currentColor", color);
        let tree = resvg::usvg::Tree::from_str(&text, &resvg::usvg::Options::default())?;
        let mut pixmap =
            resvg::tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE).context("an empty icon")?;
        let scale = ICON_SIZE as f32 / tree.size().width().max(tree.size().height());
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        Ok((demultiply(pixmap.take()), ICON_SIZE, ICON_SIZE))
    };
    match image {
        Image::Icon(name) | Image::TintedIcon(name, _) => {
            let data = gpui_kit::assets::AllAssets
                .load(&format!("icons/{name}.svg"))?
                .with_context(|| format!("there is no icon `{name}`"))?;
            svg(data.into_owned())
        }
        Image::File(path) => {
            let path = root.join(path);
            let data =
                std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
            if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
            {
                svg(data)
            } else {
                let pixmap = resvg::tiny_skia::Pixmap::decode_png(&data)
                    .with_context(|| format!("{} is not a PNG", path.display()))?;
                let (width, height) = (pixmap.width(), pixmap.height());
                Ok((demultiply(pixmap.take()), width, height))
            }
        }
        Image::Circle(inner) => icon_pixels(inner, root, color),
        other => anyhow::bail!("a tray icon cannot be {other:?}"),
    }
}

/// tiny-skia keeps premultiplied alpha; icons take it straight.
fn demultiply(mut pixels: Vec<u8>) -> Vec<u8> {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    pixels
}

/// The tooltip: the title, the tooltip, and whether it is loading, as the
/// platform without a title beside the icon shows them.
pub fn tooltip(model: &MenuBarModel, fallback: &SharedString) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(title) = model.title() {
        parts.push(title.to_string());
    }
    match model.tooltip() {
        Some(tooltip) => parts.push(tooltip.to_string()),
        None if parts.is_empty() => parts.push(fallback.to_string()),
        None => {}
    }
    if model.is_loading() {
        parts.push("Loading…".into());
    }
    parts.join(" — ")
}

/// Every entry a menu holds, depth first, in the order [`Tray`] numbers them.
pub fn numbered_items(model: &MenuBarModel) -> Vec<&crate::model::MenuBarItem> {
    fn walk<'a>(entries: &'a [MenuBarEntry], out: &mut Vec<&'a crate::model::MenuBarItem>) {
        for entry in entries {
            match entry {
                MenuBarEntry::Item(item) => out.push(item),
                MenuBarEntry::Submenu { entries, .. } => walk(entries, out),
                MenuBarEntry::Separator => {}
            }
        }
    }
    let mut out = Vec::new();
    for section in model.sections() {
        walk(section.entries(), &mut out);
    }
    out
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform {
    use std::{
        path::Path,
        sync::{Mutex, OnceLock, PoisonError},
    };

    use anyhow::Result;
    use gpui_kit::SharedString;
    use tray_icon::{
        Icon, TrayIcon, TrayIconBuilder,
        menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    };

    use super::{icon_pixels, tooltip};
    use crate::model::{Image, MenuBarEntry, MenuBarModel};

    pub struct Tray {
        id: String,
        icon: TrayIcon,
    }

    impl Tray {
        /// Shows a tray icon for `id`, a command, with what `model` holds.
        pub fn new(
            id: &str,
            model: &MenuBarModel,
            fallback_icon: &Image,
            root: &Path,
            title: &SharedString,
        ) -> Result<Self> {
            let builder = TrayIconBuilder::new()
                .with_id(id)
                .with_menu(Box::new(menu(id, model)?))
                .with_tooltip(tooltip(model, title))
                .with_menu_on_left_click(true)
                .with_icon_as_template(cfg!(target_os = "macos"));
            let builder = match icon(model, fallback_icon, root) {
                Some(icon) => builder.with_icon(icon),
                None => builder,
            };
            let builder = match model.title() {
                Some(text) if cfg!(target_os = "macos") => builder.with_title(text),
                _ => builder,
            };
            Ok(Self {
                id: id.to_owned(),
                icon: builder.build()?,
            })
        }

        /// Shows a newer render of the command.
        pub fn update(
            &self,
            model: &MenuBarModel,
            fallback_icon: &Image,
            root: &Path,
            title: &SharedString,
        ) -> Result<()> {
            self.icon.set_menu(Some(Box::new(menu(&self.id, model)?)));
            self.icon.set_tooltip(Some(tooltip(model, title)))?;
            self.icon.set_icon(icon(model, fallback_icon, root))?;
            if cfg!(target_os = "macos") {
                self.icon.set_title(model.title());
            }
            Ok(())
        }
    }

    /// Forwards menu choices as the tray's id and the entry's number, or
    /// `None` for Remove from Tray; set once.
    pub fn menu_events(on_choose: impl Fn(String, Option<usize>) + Send + Sync + 'static) {
        subscribe_menu_events(move |id| {
            let Some((tray, entry)) = id.rsplit_once('#') else {
                return;
            };
            match entry {
                REMOVE => on_choose(tray.to_owned(), None),
                number => {
                    if let Ok(number) = number.parse() {
                        on_choose(tray.to_owned(), Some(number));
                    }
                }
            }
        });
    }

    type MenuHandler = Box<dyn Fn(&str) + Send + Sync>;

    /// Calls `handler` with the id of every menu entry chosen in any tray.
    ///
    /// The platform takes one handler for all menus, so the commands' trays
    /// and the launcher's own each subscribe here and skip ids not theirs.
    pub fn subscribe_menu_events(handler: impl Fn(&str) + Send + Sync + 'static) {
        static HANDLERS: OnceLock<Mutex<Vec<MenuHandler>>> = OnceLock::new();
        let handlers = HANDLERS.get_or_init(|| {
            MenuEvent::set_event_handler(Some(|event: MenuEvent| {
                if let Some(handlers) = HANDLERS.get() {
                    for handler in handlers
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .iter()
                    {
                        handler(&event.id.0);
                    }
                }
            }));
            Mutex::default()
        });
        handlers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(handler));
    }

    /// The color tray icons are drawn in: black for the macOS template
    /// image and a light taskbar, white for a dark one.
    pub fn icon_color() -> &'static str {
        if cfg!(target_os = "macos") || !taskbar_is_dark() {
            "#000000"
        } else {
            "#ffffff"
        }
    }

    const REMOVE: &str = "remove";

    fn icon(model: &MenuBarModel, fallback: &Image, root: &Path) -> Option<Icon> {
        let color = icon_color();
        let image = model.icon().unwrap_or(fallback);
        let pixels = icon_pixels(image, root, color)
            .or_else(|error| {
                tracing::warn!("cannot draw the tray icon: {error:#}");
                icon_pixels(&Image::Icon("app-window".into()), root, color)
            })
            .ok()?;
        Icon::from_rgba(pixels.0, pixels.1, pixels.2).ok()
    }

    fn menu(id: &str, model: &MenuBarModel) -> Result<Menu> {
        let menu = Menu::new();
        let mut number = 0;
        for (ix, section) in model.sections().iter().enumerate() {
            if ix > 0 {
                menu.append(&PredefinedMenuItem::separator())?;
            }
            if let Some(title) = section.title() {
                menu.append(&MenuItem::new(title, false, None))?;
            }
            for entry in section.entries() {
                append(&menu, id, entry, &mut number)?;
            }
        }
        if model.sections().is_empty() {
            menu.append(&MenuItem::new(
                match model.is_loading() {
                    true => "Loading…",
                    false => "Nothing to show",
                },
                false,
                None,
            ))?;
        }
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&MenuItem::with_id(
            format!("{id}#{REMOVE}"),
            "Remove from Tray",
            true,
            None,
        ))?;
        Ok(menu)
    }

    /// Something entries can be appended to: the menu or a submenu.
    trait Container {
        fn add(&self, item: &dyn tray_icon::menu::IsMenuItem) -> tray_icon::menu::Result<()>;
    }

    impl Container for Menu {
        fn add(&self, item: &dyn tray_icon::menu::IsMenuItem) -> tray_icon::menu::Result<()> {
            self.append(item)
        }
    }

    impl Container for Submenu {
        fn add(&self, item: &dyn tray_icon::menu::IsMenuItem) -> tray_icon::menu::Result<()> {
            self.append(item)
        }
    }

    fn append(
        container: &dyn Container,
        id: &str,
        entry: &MenuBarEntry,
        number: &mut usize,
    ) -> Result<()> {
        match entry {
            MenuBarEntry::Item(item) => {
                let item_id = format!("{id}#{number}");
                *number += 1;
                // Text after a tab is drawn at the end of the line, where a
                // shortcut would be.
                let text = match item.subtitle() {
                    Some(subtitle) => format!("{}\t{subtitle}", item.title()),
                    None => item.title().to_string(),
                };
                let enabled = item.action().is_some();
                match item.checked() {
                    Some(checked) => container.add(&CheckMenuItem::with_id(
                        item_id, text, enabled, checked, None,
                    ))?,
                    None => container.add(&MenuItem::with_id(item_id, text, enabled, None))?,
                }
            }
            MenuBarEntry::Separator => container.add(&PredefinedMenuItem::separator())?,
            MenuBarEntry::Submenu { title, entries } => {
                let submenu = Submenu::new(title, true);
                for entry in entries {
                    append(&submenu, id, entry, number)?;
                }
                container.add(&submenu)?;
            }
        }
        Ok(())
    }

    /// Whether the taskbar is drawn dark, as Windows draws it by default.
    #[cfg(target_os = "windows")]
    fn taskbar_is_dark() -> bool {
        use windows::{
            Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
            core::w,
        };
        let mut light = 0u32;
        let mut size = size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                w!("SystemUsesLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut light as *mut u32 as *mut _),
                Some(&mut size),
            )
        };
        status.is_err() || light == 0
    }

    #[cfg(not(target_os = "windows"))]
    fn taskbar_is_dark() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Action, Effect, MenuBarItem, MenuBarSection};

    #[test]
    fn test_draws_a_lucide_icon() {
        let (pixels, width, height) =
            icon_pixels(&Image::Icon("bell".into()), Path::new("."), "#ffffff").unwrap();
        assert_eq!((width, height), (ICON_SIZE, ICON_SIZE));
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel == [255, 255, 255, 255])
        );
        assert!(icon_pixels(&Image::Icon("no-such-icon".into()), Path::new("."), "#fff").is_err());
    }

    #[test]
    fn test_numbers_items_depth_first() {
        let model = MenuBarModel::new()
            .with_title("3")
            .with_entry(MenuBarEntry::Item(
                MenuBarItem::new("A").with_action(Action::new("A", Effect::Pop)),
            ))
            .with_entry(MenuBarEntry::Submenu {
                title: "More".into(),
                entries: vec![
                    MenuBarEntry::Item(MenuBarItem::new("B")),
                    MenuBarEntry::Separator,
                    MenuBarEntry::Item(MenuBarItem::new("C")),
                ],
            })
            .with_section(
                MenuBarSection::new()
                    .with_title("Later")
                    .with_entry(MenuBarEntry::Item(MenuBarItem::new("D"))),
            );
        let titles: Vec<&str> = numbered_items(&model)
            .iter()
            .map(|item| item.title().as_ref())
            .collect();
        assert_eq!(titles, ["A", "B", "C", "D"]);
        assert_eq!(tooltip(&model, &"Inbox".into()), "3");
        assert_eq!(
            tooltip(&MenuBarModel::new().with_loading(true), &"Inbox".into()),
            "Inbox — Loading…"
        );
    }
}
