//! One object in full: its Markdown body beside labelled metadata.

use gpui_kit::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, RenderOnce, ScrollHandle, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, WeakEntity, Window,
    component::{ActiveTheme as _, h_flex, link::Link, scroll::Scrollbar, text::TextView, v_flex},
    div, img,
    prelude::FluentBuilder as _,
    rems,
};

use super::{LauncherWindow, picture::tag};
use crate::model::{DetailModel, Effect, Metadata, MetadataValue};

/// Draws a [`DetailModel`] as a page of its own or as the side pane of a
/// list. The body scrolls; the metadata column, which is short by nature,
/// sits beside it (or below it in the narrower side pane).
#[derive(IntoElement)]
pub(super) struct DetailView {
    /// Keys the parsed Markdown, so the body is parsed once per object.
    id: ElementId,
    detail: DetailModel,
    scroll: ScrollHandle,
    compact: bool,
    launcher: WeakEntity<LauncherWindow>,
}

impl DetailView {
    pub(super) fn new(
        id: impl Into<ElementId>,
        detail: DetailModel,
        scroll: &ScrollHandle,
        launcher: WeakEntity<LauncherWindow>,
    ) -> Self {
        Self {
            id: id.into(),
            detail,
            scroll: scroll.clone(),
            compact: false,
            launcher,
        }
    }

    /// Stacks the metadata under the body, for the side pane of a list.
    pub(super) fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }
}

impl RenderOnce for DetailView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let has_text = !self.detail.markdown().trim().is_empty();
        let has_body = has_text || self.detail.image().is_some();
        let has_metadata = !self.detail.metadata().is_empty();
        let metadata = has_metadata.then(|| {
            metadata_column(
                self.detail.metadata(),
                self.compact,
                self.launcher.clone(),
                cx,
            )
        });
        let body = has_body.then(|| {
            v_flex()
                .gap_4()
                .when_some(self.detail.image().cloned(), |this, image| {
                    this.child(
                        div()
                            .flex()
                            .justify_center()
                            .p_2()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().muted)
                            .child(
                                img(image)
                                    .max_w_full()
                                    .max_h(rems(16.))
                                    .object_fit(ObjectFit::Contain),
                            ),
                    )
                })
                .when(has_text, |this| {
                    this.child(
                        TextView::markdown(self.id.clone(), self.detail.markdown().clone())
                            .selectable(true),
                    )
                })
                .into_any_element()
        });

        // One scroll owner for the whole pane: the body and, in the compact
        // layout, the metadata under it scroll together.
        let scroll_area = |content: AnyElement| {
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(
                    div()
                        .id("detail-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(content),
                )
                .child(Scrollbar::vertical(&self.scroll))
        };

        match self.compact {
            // The side pane is narrow; smaller text keeps a line of code or
            // prose on one line.
            true => scroll_area(
                v_flex()
                    .p_4()
                    .gap_4()
                    .text_sm()
                    .children(body)
                    .when_some(metadata, |this, metadata| {
                        this.when(has_body, |this| {
                            this.child(div().h_px().bg(cx.theme().border))
                        })
                        .child(metadata)
                    })
                    .into_any_element(),
            )
            .into_any_element(),
            false => h_flex()
                .items_stretch()
                .size_full()
                .when(has_body || !has_metadata, |this| {
                    this.child(scroll_area(div().p_4().children(body).into_any_element()))
                })
                .when_some(metadata, |this, metadata| {
                    this.child(
                        div()
                            .id("detail-metadata")
                            .flex_none()
                            .w_64()
                            .p_4()
                            .border_l_1()
                            .border_color(cx.theme().border)
                            .overflow_y_scroll()
                            .child(metadata),
                    )
                })
                .into_any_element(),
        }
    }
}

/// The metadata as label-over-value stacks, or, in the compact side pane,
/// as one row per entry with the label on the left and the value on the
/// right.
fn metadata_column(
    metadata: &[Metadata],
    compact: bool,
    launcher: WeakEntity<LauncherWindow>,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_3()
        .text_sm()
        .children(metadata.iter().enumerate().map(|(ix, entry)| {
            let label = div()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .when(!compact, |this| this.text_xs())
                .child(entry.label().clone());
            let row = |value: AnyElement| match compact {
                true => h_flex()
                    .justify_between()
                    .gap_4()
                    .child(label)
                    .child(div().min_w_0().truncate().child(value))
                    .into_any_element(),
                false => v_flex()
                    .gap_1()
                    .child(label)
                    .child(value)
                    .into_any_element(),
            };
            match entry.value() {
                MetadataValue::Separator => div().h_px().bg(cx.theme().border).into_any_element(),
                MetadataValue::Text(text) => row(div().child(text.clone()).into_any_element()),
                MetadataValue::Link { text, url } => {
                    let url = url.clone();
                    let launcher = launcher.clone();
                    row(Link::new(("metadata-link", ix))
                        .child(text.clone())
                        .on_click(move |_, window, cx| {
                            let url = url.clone();
                            launcher
                                .update(cx, |launcher, cx| {
                                    launcher.perform_effect(Effect::OpenUrl(url), window, cx)
                                })
                                .ok();
                        })
                        .into_any_element())
                }
                MetadataValue::Tags(tags) => row(h_flex()
                    .flex_wrap()
                    .when(compact, |this| this.justify_end())
                    .gap_1()
                    .children(
                        tags.iter()
                            .map(|value| tag(value.text().clone(), value.tone())),
                    )
                    .into_any_element()),
            }
        }))
        .into_any_element()
}
