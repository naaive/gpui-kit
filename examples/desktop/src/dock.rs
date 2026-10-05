use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::apps::{AppCatalog, DesktopApp};
use crate::compositor::{Compositor, WindowIdentity};

/// Geometry in rems. The dock's layer surface is sized from these, because
/// `Root` paints the whole window and the surface must hug the items.
const SLOT: f32 = 3.;
const ICON: f32 = 2.5;
const GAP: f32 = 0.25;
const PADDING: f32 = 0.25;
pub const DOCK_HEIGHT: f32 = 4.;

/// One icon in the dock: a favorite, or an open window that no favorite owns.
#[derive(Clone)]
struct DockItem {
    id: SharedString,
    name: SharedString,
    icon: Option<PathBuf>,
    command_line: Option<String>,
    window: Option<WindowIdentity>,
}

impl DockItem {
    fn for_app(app: &DesktopApp, window: Option<WindowIdentity>) -> Self {
        Self {
            id: app.id().clone(),
            name: app.name().clone(),
            icon: app.icon().cloned(),
            command_line: Some(app.command_line().to_string()),
            window,
        }
    }
}

/// The dock along the bottom edge: favorites first, then other open
/// applications, each marked when it has a window.
pub struct Dock {
    catalog: Rc<AppCatalog>,
    favorites: Vec<DesktopApp>,
    compositor: Entity<Compositor>,
    items: Vec<DockItem>,
    _subscriptions: Vec<Subscription>,
}

impl Dock {
    pub fn new(
        catalog: Rc<AppCatalog>,
        compositor: Entity<Compositor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let favorites = catalog.favorites();
        let subscriptions = vec![cx.observe_in(&compositor, window, |this, _, window, cx| {
            this.refresh_items(window, cx);
        })];
        let mut dock = Self {
            catalog,
            favorites,
            compositor,
            items: Vec::new(),
            _subscriptions: subscriptions,
        };
        dock.items = dock.collect_items(cx);
        dock
    }

    /// The layer surface's size for `item_count` icons.
    pub fn surface_size(item_count: usize, rem: Pixels) -> Size<Pixels> {
        let count = item_count.max(1) as f32;
        size(
            rem * (count * SLOT + (count - 1.) * GAP + 2. * PADDING),
            rem * DOCK_HEIGHT,
        )
    }

    fn collect_items(&self, cx: &App) -> Vec<DockItem> {
        let windows = self.compositor.read(cx).windows();
        let mut items: Vec<DockItem> = self
            .favorites
            .iter()
            .map(|app| {
                let window = windows.iter().find(|window| app.owns(window)).cloned();
                DockItem::for_app(app, window)
            })
            .collect();

        for window in windows {
            if items
                .iter()
                .any(|item| item.window.as_ref() == Some(window))
            {
                continue;
            }
            let item = match self.catalog.owner_of(window) {
                Some(app) => DockItem::for_app(app, Some(window.clone())),
                None => DockItem {
                    id: SharedString::from(window.name().to_string()),
                    name: SharedString::from(window.name().to_string()),
                    icon: None,
                    command_line: None,
                    window: Some(window.clone()),
                },
            };
            // One icon per application, however many windows it has.
            if !items.iter().any(|existing| existing.id == item.id) {
                items.push(item);
            }
        }
        items
    }

    fn refresh_items(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.collect_items(cx);
        if items.len() != self.items.len() {
            window.resize(Self::surface_size(items.len(), window.rem_size()));
        }
        self.items = items;
        cx.notify();
    }

    /// Focuses the application's window, or starts it when it has none.
    fn activate(&self, item: &DockItem, cx: &mut App) {
        self.compositor.update(cx, |compositor, cx| {
            if let Some(window) = &item.window {
                compositor.focus_window(window, cx);
            } else if let Some(command_line) = &item.command_line {
                compositor.launch(command_line.clone(), cx);
            }
        });
    }
}

impl Render for Dock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .justify_center()
            .p(rems(PADDING))
            .gap(rems(GAP))
            .border_1()
            .border_color(cx.theme().border)
            .children(self.items.iter().map(|item| {
                let target = item.clone();
                let is_running = item.window.is_some();
                v_flex()
                    .items_center()
                    .gap_0p5()
                    .child(
                        Button::new(SharedString::from(format!("dock-{}", item.id)))
                            .ghost()
                            .size(rems(SLOT))
                            .accessibility_label(item.name.clone())
                            .child(match &item.icon {
                                Some(path) => img(path.clone()).size(rems(ICON)).into_any_element(),
                                None => Icon::new(IconName::AppWindow)
                                    .size(rems(ICON))
                                    .into_any_element(),
                            })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.activate(&target, cx)),
                            ),
                    )
                    // The indicator keeps its slot when hidden, so icons stay on
                    // one line whether or not their application is running.
                    .child(
                        div()
                            .size_1()
                            .rounded_full()
                            .when(is_running, |this| this.bg(cx.theme().foreground)),
                    )
            }))
    }
}
