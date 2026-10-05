use chrono::Local;
use gpui_kit::component::calendar::{Calendar, CalendarState};
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, v_flex};
use gpui_kit::*;

use crate::popup::popup_surface;

/// The date menu that drops from the top bar's clock.
pub struct CalendarPopup {
    calendar: Entity<CalendarState>,
    focus_handle: FocusHandle,
}

impl CalendarPopup {
    /// Fits the heading and a month of six weeks, the most a month spans.
    pub fn size(rem: Pixels) -> Size<Pixels> {
        size(rem * 20., rem * 22.)
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        Self {
            calendar: cx.new(|cx| CalendarState::new(window, cx)),
            focus_handle,
        }
    }
}

impl Render for CalendarPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let today = Local::now();
        popup_surface(cx)
            .track_focus(&self.focus_handle)
            .on_key_down(|event, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            })
            .p_3()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(today.format("%A").to_string()),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .child(today.format("%B %-d, %Y").to_string()),
                    ),
            )
            .child(Calendar::new(&self.calendar).border_0().p_0())
    }
}
