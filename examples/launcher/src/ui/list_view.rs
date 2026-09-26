//! Lists and grids: section headers, items, their empty state, and the
//! detail pane beside the selection.
//!
//! Only the lines in view are drawn, so a list of every installed
//! application scrolls as cheaply as a list of five. Line heights are derived
//! from the window's `rem`, which keeps the virtual list's measurements and
//! the drawn rows on one scale when the interface zooms.

use std::{ops::Range, rc::Rc};

use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, MouseMoveEvent,
    ParentElement as _, Pixels, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window,
    component::{
        ActiveTheme as _, h_flex, scroll::Scrollbar, tooltip::Tooltip, v_flex, v_virtual_list,
    },
    div,
    prelude::FluentBuilder as _,
    px, relative, size,
};

use super::{
    LauncherWindow,
    detail_view::DetailView,
    keyed_id,
    picture::{PictureSize, picture, tag},
};
use crate::{
    model::{Accessory, Item, ItemId, ListModel},
    session::{Line, Row, Rows},
};

/// The rows drawn in the current frame, shared with the virtual list, which
/// asks for the lines in view after `render` has returned.
#[derive(Default)]
pub(super) struct Frame {
    rows: Rows,
    selected: Option<usize>,
    geometry: LineGeometry,
}

impl Frame {
    pub(super) fn new(rows: Rows, selected: Option<usize>, window: &Window) -> Self {
        let geometry = LineGeometry::new(rows.columns(), window);
        Self {
            rows,
            selected,
            geometry,
        }
    }

    pub(super) fn rows(&self) -> &Rows {
        &self.rows
    }

    pub(super) fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub(super) fn selected_item(&self) -> Option<&Item> {
        self.selected.and_then(|ix| self.rows.item(ix))
    }
}

/// Line geometry, in `rem`.
const ITEM_LINE_REMS: f32 = 2.5;
const HEADER_LINE_REMS: f32 = 2.;
/// The inset of the list on every side, and between grid cells.
const LIST_INSET_REMS: f32 = 0.5;
/// The title under a grid cell's picture, with the gap above it.
const CELL_TITLE_REMS: f32 = 1.75;

/// The heights the virtual list measures by and the lines are drawn at, so
/// the two always agree.
#[derive(Clone, Copy, Default)]
struct LineGeometry {
    inset: Pixels,
    item: Pixels,
    header: Pixels,
    /// A grid cell's square picture area; zero in a list.
    square: Pixels,
    cell_title: Pixels,
}

impl LineGeometry {
    fn new(columns: Option<usize>, window: &Window) -> Self {
        let rem = window.rem_size();
        let inset = rem * LIST_INSET_REMS;
        let square = columns.map_or(px(0.), |columns| {
            let width = window.viewport_size().width - inset * 2.;
            (width - inset * (columns as f32 - 1.)) / columns as f32
        });
        Self {
            inset,
            item: rem * ITEM_LINE_REMS,
            header: rem * HEADER_LINE_REMS,
            square,
            cell_title: rem * CELL_TITLE_REMS,
        }
    }

    fn cell(&self) -> Pixels {
        self.square + self.cell_title
    }
}

impl LauncherWindow {
    /// Draws a list page's body.
    pub(super) fn render_list(&mut self, list: &ListModel, cx: &mut Context<Self>) -> AnyElement {
        if !self.frame.rows().has_items() {
            if list.is_loading() {
                // The progress line says why nothing is here yet; an empty
                // state now would flash before the results arrive.
                return div().into_any_element();
            }
            let title = list.empty_title().cloned().unwrap_or("No Results".into());
            return notice(title, list.empty_description().cloned(), cx);
        }

        let lines = self.render_lines(cx);
        let showing_detail = list.is_showing_detail() && self.frame.rows().columns().is_none();
        if !showing_detail {
            return lines;
        }
        let detail = self.frame.selected_item().map(|item| {
            let id = item.id().clone();
            (id, item.detail().cloned())
        });
        h_flex()
            .items_stretch()
            .size_full()
            .child(div().flex_none().w_2_5().h_full().child(lines))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .when_some(detail, |this, (id, detail)| match detail {
                        Some(detail) => this.child(
                            DetailView::new(
                                keyed_id("item-detail", id.as_shared().clone()),
                                detail,
                                &self.detail_scroll,
                                cx.entity().downgrade(),
                            )
                            .compact(true),
                        ),
                        None => this,
                    }),
            )
            .into_any_element()
    }

    fn render_lines(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let geometry = self.frame.geometry;
        let columns = self.frame.rows().columns();
        let sizes: Vec<_> = self
            .frame
            .rows()
            .lines()
            .iter()
            .map(|line| {
                let height = match (line, columns) {
                    (Line::Header(_), _) => geometry.header,
                    (Line::Items(_), None) => geometry.item,
                    (Line::Items(_), Some(_)) => geometry.cell(),
                };
                size(px(0.), height)
            })
            .collect();

        div()
            .id("launcher-list")
            .role(Role::ListBox)
            .aria_label("Results")
            .relative()
            .size_full()
            .child(
                v_virtual_list(
                    cx.entity(),
                    "launcher-lines",
                    Rc::new(sizes),
                    |this, range, _, cx| this.draw_lines(range, cx),
                )
                .p(geometry.inset)
                .track_scroll(&self.list_scroll),
            )
            .child(Scrollbar::vertical(&self.list_scroll))
            .into_any_element()
    }

    fn draw_lines(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let frame = self.frame.clone();
        range
            .filter_map(|line_ix| frame.rows().lines().get(line_ix))
            .map(|line| match line {
                Line::Header(row_ix) => match &frame.rows().rows()[*row_ix] {
                    Row::Header { title, subtitle } => {
                        section_header(title.clone(), subtitle.clone(), frame.geometry.header, cx)
                    }
                    Row::Item(_) => div().into_any_element(),
                },
                Line::Items(range) => match frame.rows().columns() {
                    None => self.item_row(range.start, &frame, cx),
                    Some(columns) => h_flex()
                        .items_start()
                        .h(frame.geometry.cell())
                        .gap(frame.geometry.inset)
                        .children(
                            range
                                .clone()
                                .map(|row_ix| self.grid_cell(row_ix, &frame, cx)),
                        )
                        // Keep a short last line's cells on the full lines' grid.
                        .children((range.len()..columns).map(|_| div().flex_1()))
                        .into_any_element(),
                },
            })
            .collect()
    }

    fn item_row(&self, row_ix: usize, frame: &Frame, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = frame.rows().item(row_ix) else {
            return div().into_any_element();
        };
        let selected = frame.selected() == Some(row_ix);
        let target = self.pointer_target(item.id(), cx);
        let theme = cx.theme();
        let muted = match selected {
            true => theme.accent_foreground.opacity(0.7),
            false => theme.muted_foreground,
        };
        target
            .role(Role::ListBoxOption)
            .aria_selected(selected)
            .aria_label(item.title().clone())
            .h(frame.geometry.item)
            .flex()
            .items_center()
            .gap_3()
            .px_2()
            .rounded(theme.radius)
            .cursor_default()
            .when(selected, |this| {
                this.bg(theme.accent).text_color(theme.accent_foreground)
            })
            .child(
                // A fixed slot, so titles keep one spine with or without pictures.
                div()
                    .flex_none()
                    .size_5()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(item.image(), |this, image| {
                        this.child(picture(image, PictureSize::Row, muted))
                    }),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.7))
                            .truncate()
                            .child(item.title().clone()),
                    )
                    .when_some(item.subtitle().cloned(), |this, subtitle| {
                        this.child(div().min_w_0().truncate().text_color(muted).child(subtitle))
                    }),
            )
            .child(
                h_flex().flex_none().gap_3().children(
                    item.accessories()
                        .iter()
                        .enumerate()
                        .map(|(ix, accessory)| accessory_element(ix, accessory, muted, cx)),
                ),
            )
            .into_any_element()
    }

    fn grid_cell(&self, row_ix: usize, frame: &Frame, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = frame.rows().item(row_ix) else {
            return div().flex_1().into_any_element();
        };
        let selected = frame.selected() == Some(row_ix);
        let target = self.pointer_target(item.id(), cx);
        let theme = cx.theme();
        target
            .role(Role::ListBoxOption)
            .aria_selected(selected)
            .aria_label(item.title().clone())
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .cursor_default()
            .child(
                div()
                    .h(frame.geometry.square)
                    .p_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme.radius_lg)
                    .bg(theme.muted)
                    .border_2()
                    .border_color(match selected {
                        true => theme.ring,
                        false => theme.transparent,
                    })
                    .when_some(item.image(), |this, image| {
                        this.child(picture(image, PictureSize::Cell, theme.foreground))
                    }),
            )
            .child(
                div()
                    .h(frame.geometry.cell_title)
                    .pt_1()
                    .px_1()
                    .text_xs()
                    .text_center()
                    .truncate()
                    .text_color(match selected {
                        true => theme.foreground,
                        false => theme.muted_foreground,
                    })
                    .child(item.title().clone()),
            )
            .into_any_element()
    }

    /// An item's pointer behavior: moving over it selects it, a click
    /// performs its primary action.
    ///
    /// Selection follows the pointer only when the pointer itself moves, so
    /// a list scrolling under a still pointer — after a key press, say —
    /// never steals the selection back.
    fn pointer_target(
        &self,
        id: &ItemId,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let hover_id = id.clone();
        let click_id = id.clone();
        div()
            .id(keyed_id("launcher-item", id.as_shared().clone()))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                this.pointer_moved(&hover_id, event, cx);
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate(click_id.clone(), window, cx);
            }))
    }
}

fn section_header(
    title: SharedString,
    subtitle: Option<SharedString>,
    height: Pixels,
    cx: &Context<LauncherWindow>,
) -> AnyElement {
    h_flex()
        .h(height)
        .items_end()
        .gap_2()
        .px_2()
        .pb_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(div().font_weight(FontWeight::MEDIUM).child(title))
        .when_some(subtitle, |this, subtitle| this.child(subtitle))
        .into_any_element()
}

fn accessory_element(
    ix: usize,
    accessory: &Accessory,
    muted: gpui_kit::Hsla,
    _: &Context<LauncherWindow>,
) -> AnyElement {
    let content = match (accessory.tone(), accessory.label()) {
        (Some(tone), Some(text)) => tag(text.clone(), tone),
        (_, text) => h_flex()
            .gap_1()
            .text_sm()
            .text_color(muted)
            .when_some(accessory.picture(), |this, image| {
                this.child(picture(image, PictureSize::Row, muted))
            })
            .when_some(text.cloned(), |this, text| this.child(text))
            .into_any_element(),
    };
    match accessory.tooltip().cloned() {
        Some(tooltip) => div()
            .id(("accessory", ix))
            .child(content)
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .into_any_element(),
        None => content,
    }
}

/// A centered message filling the body: an empty list, or why a page failed.
pub(super) fn notice(
    title: SharedString,
    message: Option<SharedString>,
    cx: &gpui_kit::App,
) -> AnyElement {
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
