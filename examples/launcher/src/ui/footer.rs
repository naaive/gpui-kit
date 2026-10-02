use gpui_kit::{
    App, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
    component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, spinner::Spinner},
    div,
    prelude::FluentBuilder as _,
    px,
};

use super::{LauncherWindow, keycaps::keycaps};

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
        let theme = cx.theme();
        h_flex()
            .flex_none()
            .justify_between()
            .gap_2()
            .h(px(42.))
            .pl_4()
            .pr_2()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.foreground.opacity(0.035))
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(
                h_flex()
                    .min_w_0()
                    .gap(px(7.))
                    .child(
                        // The launcher's mark, so the page's name reads as
                        // where you are in it.
                        div()
                            .flex_none()
                            .size(px(14.))
                            .rounded(px(4.))
                            .bg(theme.foreground)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Search)
                                    .size(px(9.))
                                    .text_color(theme.background),
                            ),
                    )
                    .child(div().truncate().child(self.title))
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .when_some(self.primary, |this, primary| {
                        this.child(
                            hint("footer-primary", primary.title, primary.keystroke, true, cx)
                                .on_click(move |_, window, cx| {
                                    primary_launcher
                                        .update(cx, |launcher, cx| {
                                            launcher.perform_primary(window, cx)
                                        })
                                        .ok();
                                }),
                        )
                    })
                    .when(self.shows_actions, |this| {
                        this.child(div().h(px(14.)).w_px().bg(theme.border)).child(
                            hint("footer-actions", "Actions".into(), "secondary-k", false, cx)
                                .on_click(move |_, window, cx| {
                                    actions_launcher
                                        .update(cx, |launcher, cx| {
                                            launcher.toggle_action_panel(window, cx)
                                        })
                                        .ok();
                                }),
                        )
                    }),
            )
    }
}

/// A command and its keys: a quiet button, since clicking it does what the
/// key does. It is not a Tab stop; the key is the keyboard path.
fn hint(
    id: &'static str,
    title: SharedString,
    keystroke: &str,
    emphasized: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let hover = theme.foreground.opacity(0.07);
    h_flex()
        .id(id)
        .role(Role::Button)
        .aria_label(title.clone())
        .flex_none()
        .h(px(28.))
        .gap(px(7.))
        .pl(px(9.))
        .pr(px(6.))
        .rounded(px(7.))
        .cursor_default()
        .hover(move |this| this.bg(hover))
        .map(|this| match emphasized {
            true => this
                .text_color(theme.foreground)
                .font_weight(FontWeight::MEDIUM),
            false => this.text_color(theme.foreground.opacity(0.78)),
        })
        .child(title)
        .when_some(keycaps(keystroke, cx), |this, caps| this.child(caps))
}
