use std::time::Duration;

use chrono::{DateTime, Local, Timelike as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar_popup::CalendarPopup;
use crate::compositor::Compositor;
use crate::popup::{close_popup, open_popup};
use crate::quick_settings::{QuickSettings, battery_icon, connection_icon, volume_icon};
use crate::system_status::SystemStatus;

/// The bar's height in rems. It holds one row of small buttons with an even
/// band above and below.
pub const TOP_BAR_HEIGHT: f32 = 2.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarMenu {
    Calendar,
    QuickSettings,
}

struct OpenMenu {
    menu: BarMenu,
    window: AnyWindowHandle,
    /// Clears `open_menu` when the popup goes away on its own: Escape, or an
    /// outside click that makes the compositor dismiss it.
    _closed: Subscription,
}

/// The bar along the top edge of one display: workspaces, the clock and the
/// system status.
pub struct TopBar {
    compositor: Entity<Compositor>,
    status: Entity<SystemStatus>,
    now: DateTime<Local>,
    open_menu: Option<OpenMenu>,
    _clock: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl TopBar {
    pub fn new(
        compositor: Entity<Compositor>,
        status: Entity<SystemStatus>,
        cx: &mut Context<Self>,
    ) -> Self {
        let clock = cx.spawn(async move |this, cx| {
            loop {
                // Wake at the next minute boundary rather than polling.
                let now = Local::now();
                let wait = 60 - u64::from(now.second());
                cx.background_executor()
                    .timer(Duration::from_secs(wait))
                    .await;
                let updated = this.update(cx, |this, cx| {
                    this.now = Local::now();
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
        });

        Self {
            _subscriptions: vec![
                cx.observe(&compositor, |_, _, cx| cx.notify()),
                cx.observe(&status, |_, _, cx| cx.notify()),
            ],
            compositor,
            status,
            now: Local::now(),
            open_menu: None,
            _clock: clock,
        }
    }

    /// Opens `menu` below the pointer, or closes it if it is already open.
    /// Opening one menu closes the other, as a menu bar does.
    fn toggle_menu(
        &mut self,
        menu: BarMenu,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(open) = self.open_menu.take()
            && close_popup(open.window, cx)
            && open.menu == menu
        {
            return;
        }

        let rem = window.rem_size();
        let anchor_rect = Bounds {
            origin: point(event.position.x, px(0.)),
            size: size(px(1.), rem * TOP_BAR_HEIGHT),
        };
        let opened = match menu {
            BarMenu::Calendar => open_popup(
                window,
                anchor_rect,
                CalendarPopup::size(rem),
                cx,
                |window, cx| cx.new(|cx| CalendarPopup::new(window, cx)),
            )
            .map(|(window, view)| (window, self.observe_menu_closed(&view, cx))),
            BarMenu::QuickSettings => {
                let status = self.status.clone();
                open_popup(
                    window,
                    anchor_rect,
                    QuickSettings::size(status.read(cx), rem),
                    cx,
                    |window, cx| cx.new(|cx| QuickSettings::new(status, window, cx)),
                )
                .map(|(window, view)| (window, self.observe_menu_closed(&view, cx)))
            }
        };
        self.open_menu = opened.map(|(window, closed)| OpenMenu {
            menu,
            window,
            _closed: closed,
        });
        cx.notify();
    }

    fn observe_menu_closed<V: 'static>(
        &self,
        view: &Entity<V>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        // The subscription lives in the `OpenMenu` it clears, so replacing the
        // menu drops it before the old popup's release can fire.
        cx.observe_release(view, |this, _, cx| {
            this.open_menu = None;
            cx.notify();
        })
    }

    fn is_open(&self, menu: BarMenu) -> bool {
        self.open_menu
            .as_ref()
            .is_some_and(|open| open.menu == menu)
    }

    fn render_workspaces(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let compositor = self.compositor.read(cx);
        h_flex()
            .flex_1()
            .gap_1()
            .children(compositor.workspaces().iter().map(|workspace| {
                let target = workspace.clone();
                Button::new(SharedString::from(format!(
                    "workspace-{}",
                    workspace.name()
                )))
                .ghost()
                .small()
                .label(workspace.name().clone())
                .selected(workspace.is_focused())
                .when(workspace.is_urgent(), |this| this.warning())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.compositor
                        .update(cx, |compositor, cx| compositor.focus_workspace(&target, cx));
                }))
            }))
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status.read(cx);
        let volume = status.volume();
        let battery = status.battery();
        let is_open = self.is_open(BarMenu::QuickSettings);

        h_flex().flex_1().justify_end().child(
            Button::new("system-status")
                .ghost()
                .small()
                .selected(is_open)
                .accessibility_label("System menu")
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(connection_icon(status.connection())).small())
                        .when_some(volume, |this, volume| {
                            this.child(
                                Icon::new(volume_icon(volume.level(), volume.is_muted())).small(),
                            )
                        })
                        .when_some(battery, |this, battery| {
                            this.child(
                                h_flex()
                                    .gap_1()
                                    .child(Icon::new(battery_icon(battery)).small())
                                    .child(format!("{}%", battery.percent())),
                            )
                        }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event, window, cx| {
                        this.toggle_menu(BarMenu::QuickSettings, event, window, cx)
                    }),
                ),
        )
    }
}

impl Render for TopBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let calendar_open = self.is_open(BarMenu::Calendar);
        h_flex()
            .size_full()
            .px_2()
            .gap_2()
            .text_sm()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.render_workspaces(cx))
            .child(
                Button::new("clock")
                    .ghost()
                    .small()
                    .selected(calendar_open)
                    .label(self.now.format("%a %b %-d  %H:%M").to_string())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| {
                            this.toggle_menu(BarMenu::Calendar, event, window, cx)
                        }),
                    ),
            )
            .child(self.render_status(cx))
    }
}
