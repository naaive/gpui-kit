//! The alert for reminders that have come due: a small window in the
//! corner of the screen that does not take the focus and stays until each
//! reminder is completed or snoozed.

use chrono::Local;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, FontWeight, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, WindowBounds, WindowKind,
    WindowOptions,
    component::{
        ActiveTheme as _, Icon, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex, v_flex,
    },
    div, point, px, size,
};

use super::{Reminder, format_due, update};

const WIDTH: f32 = 340.;
const HEIGHT: f32 = 128.;
/// Distance from the corner of the display's visible area.
const MARGIN: f32 = 16.;

/// The alert on screen and the reminder it shows first.
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

/// Shows the alert for `due`, the reminders due now, oldest first; closes
/// it when there are none.
pub(super) fn show(mut due: Vec<Reminder>, cx: &mut App) {
    close(cx);
    if due.is_empty() {
        return;
    }
    due.sort_by_key(|reminder| reminder.due);
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
    let first = due[0].id.clone();
    match gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| AlertView { due })) {
        Ok((window, _)) => cx.set_global(ShownAlert(Some((window, first)))),
        Err(error) => tracing::warn!("cannot show the reminder alert: {error:#}"),
    }
}

/// The id of the reminder the alert shows first, if one is shown.
#[cfg(test)]
pub(super) fn shown(cx: &mut App) -> Option<String> {
    cx.default_global::<ShownAlert>()
        .0
        .as_ref()
        .map(|(_, id)| id.clone())
}

/// The Complete button.
pub(super) fn complete(id: String, cx: &mut App) {
    update(cx, |reminders, cx| reminders.set_completed(&id, true, cx));
    refresh(cx);
}

/// The Snooze 10 Minutes button.
pub(super) fn snooze(id: String, cx: &mut App) {
    update(cx, |reminders, cx| reminders.snooze(&id, 10, cx));
    refresh(cx);
}

/// After a button: the alert for what is still due, or none.
fn refresh(cx: &mut App) {
    let now = super::now();
    let store = super::store(cx);
    let due: Vec<Reminder> = store
        .read(cx)
        .reminders()
        .iter()
        .filter(|reminder| reminder.is_due(now))
        .cloned()
        .collect();
    cx.defer(move |cx| show(due, cx));
}

struct AlertView {
    due: Vec<Reminder>,
}

impl Render for AlertView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let Some(reminder) = self.due.first() else {
            return div();
        };
        let more = self.due.len() - 1;
        let when: SharedString = match reminder.due_time() {
            Some(due) => format!("Due {}", format_due(due, Local::now())).into(),
            None => "Due now".into(),
        };
        let (complete, snooze) = (reminder.id.clone(), reminder.id.clone());
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
                        .child(Icon::empty().path("icons/alarm-clock.svg").small())
                        .child(match more {
                            0 => SharedString::from("Reminder"),
                            more => format!("Reminder · {more} more due").into(),
                        })
                        .child(div().flex_1())
                        .child(
                            Button::new("dismiss")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Dismiss")
                                .on_click(|_, _, cx| close(cx)),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(reminder.title.clone()),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(when),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("snooze")
                                .small()
                                .label("Snooze 10 Minutes")
                                .on_click(move |_, _, cx| self::snooze(snooze.clone(), cx)),
                        )
                        .child(
                            Button::new("complete")
                                .small()
                                .primary()
                                .label("Complete")
                                .on_click(move |_, _, cx| self::complete(complete.clone(), cx)),
                        ),
                ),
        )
    }
}
