//! The alert for timers that have ended: a small window in the corner of
//! the screen that does not take the focus and stays until each timer is
//! dismissed, restarted or given more time.

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, FontWeight, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, WindowBounds, WindowKind,
    WindowOptions,
    component::{
        ActiveTheme as _, Icon, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex, v_flex,
    },
    div, point, px, size,
};

use super::{Timer, format_time, now, update};

const WIDTH: f32 = 340.;
const HEIGHT: f32 = 128.;
/// Distance from the corner of the display's visible area.
const MARGIN: f32 = 16.;
/// What +5 Minutes adds, in milliseconds.
const EXTRA: i64 = 5 * 60_000;

/// The alert on screen and the timer it shows first.
#[derive(Default)]
struct ShownAlert(Option<(AnyWindowHandle, String)>);

impl Global for ShownAlert {}

fn close(cx: &mut App) {
    if let Some((window, _)) = cx.default_global::<ShownAlert>().0.take() {
        window
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

/// Shows the alert for `ended`, the timers that have ended, soonest ended
/// first; closes it when there are none.
pub(super) fn show(ended: Vec<Timer>, cx: &mut App) {
    close(cx);
    let Some(first) = ended.first().map(|timer| timer.id.clone()) else {
        return;
    };
    let Some(display) = cx.primary_display() else {
        return;
    };
    let visible = display.visible_bounds();
    let alert_size = size(px(WIDTH), px(HEIGHT));
    let origin = point(
        visible.right() - alert_size.width - px(MARGIN),
        visible.bottom() - alert_size.height - px(MARGIN),
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, alert_size))),
        titlebar: None,
        // It must not take the keyboard from what the user is typing in.
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display.id()),
        app_id: Some("gpui-kit-launcher".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| AlertView { ended })) {
        Ok((window, _)) => cx.set_global(ShownAlert(Some((window, first)))),
        Err(error) => tracing::warn!("cannot show the timer alert: {error:#}"),
    }
}

/// The id of the timer the alert shows first, if one is shown.
#[cfg(test)]
pub(super) fn shown(cx: &mut App) -> Option<String> {
    cx.default_global::<ShownAlert>()
        .0
        .as_ref()
        .map(|(_, id)| id.clone())
}

/// The Dismiss button: the timer is done with.
pub(super) fn dismiss(id: String, cx: &mut App) {
    update(cx, |timers, cx| timers.remove(&id, cx));
    refresh(cx);
}

/// The Restart button.
pub(super) fn restart(id: String, cx: &mut App) {
    update(cx, |timers, cx| timers.change(&id, Timer::restart, cx));
    refresh(cx);
}

/// The +5 Minutes button.
pub(super) fn extend(id: String, cx: &mut App) {
    update(cx, |timers, cx| {
        timers.change(&id, |timer, now| timer.extend(EXTRA, now), cx)
    });
    refresh(cx);
}

/// After a button: the alert for what has still ended, or none.
fn refresh(cx: &mut App) {
    let ended = super::store(cx).read(cx).ended(now());
    cx.defer(move |cx| show(ended, cx));
}

struct AlertView {
    ended: Vec<Timer>,
}

impl Render for AlertView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let Some(timer) = self.ended.first() else {
            return div();
        };
        let more = self.ended.len() - 1;
        let done: SharedString = match timer.ends_at() {
            Some(at) => format!("Done at {}", format_time(at, now())).into(),
            None => "Done".into(),
        };
        let (dismiss, restart, extend) = (timer.id.clone(), timer.id.clone(), timer.id.clone());
        div().size_full().child(
            v_flex()
                .size_full()
                .p_3()
                .gap_2()
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius)
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Icon::empty().path("icons/timer.svg").small())
                        .child(match more {
                            0 => SharedString::from("Timer"),
                            more => format!("Timer · {more} more done").into(),
                        }),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(timer.title()),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(done),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("restart")
                                .small()
                                .label("Restart")
                                .on_click(move |_, _, cx| self::restart(restart.clone(), cx)),
                        )
                        .child(
                            Button::new("extend")
                                .small()
                                .label("+5 Minutes")
                                .on_click(move |_, _, cx| self::extend(extend.clone(), cx)),
                        )
                        .child(
                            Button::new("dismiss")
                                .small()
                                .primary()
                                .label("Dismiss")
                                .on_click(move |_, _, cx| self::dismiss(dismiss.clone(), cx)),
                        ),
                ),
        )
    }
}
