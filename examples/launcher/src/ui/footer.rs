use gpui_kit::{
    App, IntoElement, Keystroke, ParentElement as _, RenderOnce, SharedString, Styled as _,
    WeakEntity, Window,
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        kbd::Kbd,
        spinner::Spinner,
    },
    div,
    prelude::FluentBuilder as _,
};

use super::LauncherWindow;

/// The action `Enter` (or, on a form, `Cmd/Ctrl-Enter`) performs.
pub(super) struct PrimaryHint {
    title: SharedString,
    keystroke: &'static str,
}

impl PrimaryHint {
    pub(super) fn new(title: SharedString, keystroke: &'static str) -> Self {
        Self { title, keystroke }
    }
}

/// The current page on the left; what the keyboard will do to the selection
/// on the right. Nothing else goes here: every footer entry refers to the
/// selected item, or on a detail or form page to the page itself.
#[derive(IntoElement)]
pub(super) struct Footer {
    title: SharedString,
    loading: bool,
    primary: Option<PrimaryHint>,
    /// Shown only when the panel lists more than the primary action, since
    /// otherwise it would open onto what the footer already says.
    shows_actions: bool,
    launcher: WeakEntity<LauncherWindow>,
}

impl Footer {
    pub(super) fn new(title: SharedString, launcher: WeakEntity<LauncherWindow>) -> Self {
        Self {
            title,
            loading: false,
            primary: None,
            shows_actions: false,
            launcher,
        }
    }

    pub(super) fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub(super) fn primary(mut self, primary: Option<PrimaryHint>) -> Self {
        self.primary = primary;
        self
    }

    pub(super) fn shows_actions(mut self, shows_actions: bool) -> Self {
        self.shows_actions = shows_actions;
        self
    }
}

impl RenderOnce for Footer {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let primary_launcher = self.launcher.clone();
        let actions_launcher = self.launcher;
        h_flex()
            .flex_none()
            .justify_between()
            .gap_4()
            .h_10()
            .px_4()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(
                h_flex()
                    .min_w_0()
                    .gap_2()
                    .child(div().truncate().child(self.title))
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(cx.theme().muted_foreground))
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .when_some(self.primary, |this, primary| {
                        this.child(
                            hint("footer-primary", primary.title, primary.keystroke).on_click(
                                move |_, window, cx| {
                                    primary_launcher
                                        .update(cx, |launcher, cx| {
                                            launcher.perform_primary(window, cx)
                                        })
                                        .ok();
                                },
                            ),
                        )
                    })
                    .when(self.shows_actions, |this| {
                        this.child(div().h_4().w_px().bg(cx.theme().border)).child(
                            hint("footer-actions", "Actions".into(), "secondary-k").on_click(
                                move |_, window, cx| {
                                    actions_launcher
                                        .update(cx, |launcher, cx| {
                                            launcher.toggle_action_panel(window, cx)
                                        })
                                        .ok();
                                },
                            ),
                        )
                    }),
            )
    }
}

/// A command and its key: a quiet button, since clicking it does what the
/// key does. It is not a Tab stop; the key is the keyboard path.
fn hint(id: &'static str, title: SharedString, keystroke: &str) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .tab_stop(false)
        .label(title)
        .when_some(Keystroke::parse(keystroke).ok(), |this, keystroke| {
            this.child(Kbd::new(keystroke))
        })
}
