//! The Services window: every console's session, what it is doing, and the
//! commands that stop it.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent},
    h_flex,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _,
};
use rust_i18n::t;

use crate::{
    console::{SessionState, Sessions, SessionsEvent},
    format,
};

pub struct ServicesPanel {
    focus_handle: FocusHandle,
    /// Redraws running times while anything runs.
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for ServicesPanel {}

impl ServicesPanel {
    pub const NAME: &str = "Services";

    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.subscribe(&Sessions::global(cx), |_, _, _: &SessionsEvent, cx| {
                cx.notify()
            }),
        ];
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            _ticker: ticker,
            _subscriptions: subscriptions,
        }
    }
}

impl Focusable for ServicesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for ServicesPanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }

    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for ServicesPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("services.title").to_string()
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for ServicesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let consoles = Sessions::global(cx).read(cx).consoles();
        let theme = cx.theme();
        if consoles.is_empty() {
            return div()
                .size_full()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t!("services.empty").to_string())
                .into_any_element();
        }
        div()
            .id("services")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .p_1()
                    .children(consoles.into_iter().enumerate().map(|(ix, console)| {
                        let panel = console.read(cx);
                        let state = panel.session_state();
                        let data_source: SharedString = panel
                            .data_source()
                            .map(|data_source| data_source.read(cx).name())
                            .unwrap_or_else(|| t!("console.data_source_missing").into());
                        let (color, status): (_, SharedString) = match state {
                            SessionState::Disconnected => {
                                (theme.muted_foreground, t!("services.disconnected").into())
                            }
                            SessionState::Idle => (theme.success, t!("services.idle").into()),
                            SessionState::InTransaction => {
                                (theme.warning, t!("services.in_transaction").into())
                            }
                            SessionState::Running {
                                started,
                                statement,
                                total,
                            } => (
                                theme.primary,
                                t!(
                                    "services.running",
                                    current = statement.max(1),
                                    total = total,
                                    duration = format::duration(started.elapsed())
                                )
                                .into(),
                            ),
                        };
                        let running = matches!(state, SessionState::Running { .. });
                        let connected = state != SessionState::Disconnected;
                        let cancel = console.downgrade();
                        let disconnect = console.downgrade();
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .rounded(theme.radius)
                            .text_sm()
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(div().flex_none().child(panel.name()))
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(data_source),
                            )
                            .child(div().size_2().rounded_full().bg(color))
                            .child(div().flex_1().min_w_0().truncate().child(status))
                            .when(running, |row| {
                                row.child(
                                    Button::new(("cancel", ix))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::CircleStop)
                                        .tooltip(t!("console.cancel").to_string())
                                        .on_click(move |_, _, cx| {
                                            let _ = cancel
                                                .update(cx, |console, cx| console.cancel_run(cx));
                                        }),
                                )
                            })
                            .when(connected, |row| {
                                row.child(
                                    Button::new(("disconnect", ix))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Unplug)
                                        .tooltip(t!("services.disconnect").to_string())
                                        .on_click(move |_, _, cx| {
                                            let _ = disconnect
                                                .update(cx, |console, cx| console.disconnect(cx));
                                        }),
                                )
                            })
                    })),
            )
            .into_any_element()
    }
}
