use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::popup::popup_surface;
use crate::system_status::{Battery, Connection, SystemStatus};

/// The system menu that drops from the top bar's status area.
pub struct QuickSettings {
    status: Entity<SystemStatus>,
    volume_slider: Entity<SliderState>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// The menu's rhythm in rems: rows as tall as a small button, separated and
/// inset by the same step.
const ROW: f32 = 1.5;
const GAP: f32 = 0.75;
const INSET: f32 = 0.75;

impl QuickSettings {
    /// The popup's size for the rows `status` will show, since an open popup
    /// cannot be resized everywhere.
    pub fn size(status: &SystemStatus, rem: Pixels) -> Size<Pixels> {
        let rows =
            1 + usize::from(status.volume().is_some()) + usize::from(status.battery().is_some());
        let rows = rows as f32;
        // The surface's hairline border adds a device pixel above and below.
        let height = rem * (2. * INSET + rows * ROW + (rows - 1.) * GAP) + px(2.);
        size(rem * 20., height)
    }

    pub fn new(status: Entity<SystemStatus>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        let level = status
            .read(cx)
            .volume()
            .map(|volume| volume.level().min(1.0))
            .unwrap_or_default();
        let volume_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.01)
                .default_value(level)
        });

        let subscriptions = vec![
            cx.observe(&status, |_, _, cx| cx.notify()),
            cx.subscribe(&volume_slider, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Change(value) = event {
                    let level = value.start();
                    this.status
                        .update(cx, |status, cx| status.set_volume_level(level, cx));
                }
            }),
        ];

        Self {
            status,
            volume_slider,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.status.update(cx, |status, cx| status.toggle_mute(cx));
    }
}

impl Render for QuickSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status.read(cx);
        let volume = status.volume();
        let connection = status.connection();
        let battery = status.battery();

        popup_surface(cx)
            .track_focus(&self.focus_handle)
            .on_key_down(|event, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            })
            .p(rems(INSET))
            .gap(rems(GAP))
            .when_some(volume, |this, volume| {
                let muted = volume.is_muted();
                this.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("mute")
                                .ghost()
                                .small()
                                .icon(Icon::new(volume_icon(volume.level(), muted)))
                                .selected(muted)
                                .accessibility_label(if muted { "Unmute" } else { "Mute" })
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))),
                        )
                        .child(Slider::new(&self.volume_slider).flex_1())
                        .child(
                            div()
                                .w_10()
                                .text_right()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{:.0}%", volume.level() * 100.)),
                        ),
                )
            })
            .child(status_row(
                connection_icon(connection),
                match connection {
                    Connection::Wired => "Wired",
                    Connection::Wireless => "Wi-Fi",
                    Connection::Offline => "Offline",
                },
                None,
                cx,
            ))
            .when_some(battery, |this, battery| {
                this.child(status_row(
                    battery_icon(battery),
                    format!("{}%", battery.percent()),
                    battery.is_charging().then_some("Charging"),
                    cx,
                ))
            })
    }
}

/// A read-only line of the menu. The icon sits in a slot as wide as the mute
/// button so every label shares one leading edge.
fn status_row(
    icon: IconName,
    label: impl Into<SharedString>,
    detail: Option<&'static str>,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .text_sm()
        .child(
            div()
                .size(rems(ROW))
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon).small()),
        )
        .child(div().flex_1().child(label.into()))
        .when_some(detail, |this, detail| {
            this.child(div().text_color(cx.theme().muted_foreground).child(detail))
        })
}

pub fn volume_icon(level: f32, muted: bool) -> IconName {
    if muted || level <= 0. {
        IconName::VolumeX
    } else if level < 0.5 {
        IconName::Volume1
    } else {
        IconName::Volume2
    }
}

pub fn connection_icon(connection: Connection) -> IconName {
    match connection {
        Connection::Wired => IconName::Network,
        Connection::Wireless => IconName::Wifi,
        Connection::Offline => IconName::WifiOff,
    }
}

pub fn battery_icon(battery: Battery) -> IconName {
    match battery.percent() {
        _ if battery.is_charging() => IconName::BatteryCharging,
        0..=10 => IconName::BatteryWarning,
        11..=35 => IconName::BatteryLow,
        36..=80 => IconName::BatteryMedium,
        _ => IconName::BatteryFull,
    }
}
