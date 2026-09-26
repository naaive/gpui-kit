use gpui_kit::{
    App, IntoElement, Keystroke, ParentElement as _, RenderOnce, SharedString, Styled as _, Window,
    component::{ActiveTheme as _, h_flex, kbd::Kbd},
    div,
    prelude::FluentBuilder as _,
};

use crate::model::{Action, Item};

/// The current page on the left; what `Enter` and `Cmd/Ctrl-Enter` will do on
/// the right. Nothing else goes here: every footer entry refers to the
/// selected item.
#[derive(IntoElement)]
pub(super) struct Footer {
    title: SharedString,
    selected: Option<Item>,
    loading: bool,
}

impl Footer {
    pub(super) fn new(title: SharedString, selected: Option<Item>, loading: bool) -> Self {
        Self {
            title,
            selected,
            loading,
        }
    }
}

impl RenderOnce for Footer {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let primary = self
            .selected
            .as_ref()
            .and_then(Item::primary_action)
            .cloned();
        let secondary = self
            .selected
            .as_ref()
            .and_then(Item::secondary_action)
            .cloned();
        h_flex()
            .flex_none()
            .justify_between()
            .gap_4()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(
                h_flex()
                    .gap_2()
                    .child(self.title)
                    .when(self.loading, |this| this.child("Loading…")),
            )
            .child(
                h_flex()
                    .gap_4()
                    .when_some(primary, |this, action| this.child(hint(&action, "enter")))
                    .when_some(secondary, |this, action| {
                        this.child(hint(&action, "secondary-enter"))
                    }),
            )
    }
}

fn hint(action: &Action, keystroke: &str) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .child(div().child(action.title().clone()))
        .when_some(Keystroke::parse(keystroke).ok(), |this, keystroke| {
            this.child(Kbd::new(keystroke))
        })
}
