use gpui_kit::{
    AnyElement, App, ElementId, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
    component::{ActiveTheme as _, Icon, h_flex, v_flex},
    div,
    prelude::FluentBuilder as _,
};

use super::LauncherWindow;
use crate::{
    model::{Item, PageModel},
    session::{Row, Rows},
};

/// The body of a page: its rows, its empty state, or why it failed.
#[derive(IntoElement)]
pub(super) struct ListView {
    model: PageModel,
    rows: Vec<Row>,
    selected: Option<usize>,
    scroll: ScrollHandle,
    launcher: Entity<LauncherWindow>,
}

impl ListView {
    pub(super) fn new(
        model: &PageModel,
        rows: &Rows,
        selected: Option<usize>,
        scroll: &ScrollHandle,
        launcher: Entity<LauncherWindow>,
    ) -> Self {
        Self {
            model: model.clone(),
            rows: rows.rows().to_vec(),
            selected,
            scroll: scroll.clone(),
            launcher,
        }
    }
}

impl RenderOnce for ListView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let list = match &self.model {
            PageModel::Failure { title, message } => {
                return notice(title.clone(), Some(message.clone()), cx);
            }
            PageModel::List(list) => list,
        };
        if !self.rows.iter().any(|row| matches!(row, Row::Item(_))) {
            if list.is_loading() {
                return div().into_any_element();
            }
            let title = list.empty_title().cloned().unwrap_or("No Results".into());
            return notice(title, None, cx);
        }

        let launcher = self.launcher.clone();
        v_flex()
            .id("launcher-rows")
            .role(Role::ListBox)
            .size_full()
            .p_2()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(
                self.rows
                    .into_iter()
                    .enumerate()
                    .map(|(ix, row)| match row {
                        Row::Header(title) => section_header(title, cx),
                        Row::Item(item) => {
                            item_row(ix, item, self.selected == Some(ix), launcher.clone(), cx)
                        }
                    }),
            )
            .into_any_element()
    }
}

fn section_header(title: SharedString, cx: &App) -> AnyElement {
    div()
        .px_2()
        .pt_2()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(title)
        .into_any_element()
}

fn item_row(
    ix: usize,
    item: Item,
    selected: bool,
    launcher: Entity<LauncherWindow>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let muted = match selected {
        true => theme.accent_foreground.opacity(0.7),
        false => theme.muted_foreground,
    };
    let id = item.id().clone();
    h_flex()
        .id(ElementId::NamedInteger("launcher-row".into(), ix as u64))
        .role(Role::ListBoxOption)
        .aria_selected(selected)
        .gap_3()
        .px_2()
        .py_1p5()
        .rounded(theme.radius)
        .cursor_default()
        .when(selected, |this| {
            this.bg(theme.accent).text_color(theme.accent_foreground)
        })
        .child(
            div()
                .size_5()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .when_some(item.icon().cloned(), |this, icon| {
                    this.child(
                        Icon::empty()
                            .path(format!("icons/{icon}.svg"))
                            .text_color(muted),
                    )
                }),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .child(div().flex_none().child(item.title().clone()))
                .when_some(item.subtitle().cloned(), |this, subtitle| {
                    this.child(div().truncate().text_color(muted).child(subtitle))
                }),
        )
        .when_some(item.accessory().cloned(), |this, accessory| {
            this.child(
                div()
                    .flex_none()
                    .text_sm()
                    .text_color(muted)
                    .child(accessory),
            )
        })
        .on_click(move |_, window, cx| {
            launcher.update(cx, |launcher, cx| launcher.activate(id.clone(), window, cx));
        })
        .into_any_element()
}

fn notice(title: SharedString, message: Option<SharedString>, cx: &App) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_1()
        .px_8()
        .child(div().text_color(cx.theme().muted_foreground).child(title))
        .when_some(
            message.filter(|message| !message.is_empty()),
            |this, message| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .text_center()
                        .child(message),
                )
            },
        )
        .into_any_element()
}
