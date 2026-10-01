//! An execution plan: each step with its estimates and, after `ANALYZE`,
//! what it really cost. The bar shows the share of the whole a step took by
//! itself, so the expensive step stands out without reading numbers.

use datakit_driver::PlanNode;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, h_flex,
    scroll::ScrollableElement as _,
    table::{Column, DataTable, TableDelegate, TableEvent, TableState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, relative, rems,
};
use rust_i18n::t;

use crate::format;

/// One row: a step, its depth, and its own share of the plan.
struct PlanRow {
    depth: usize,
    node: PlanNode,
    share: f32,
}

struct PlanGrid {
    rows: Vec<PlanRow>,
    columns: Vec<Column>,
}

impl PlanGrid {
    fn new(plan: &PlanNode, window: &Window) -> Self {
        let rem = window.rem_size();
        let analyzed = plan.actual_time().is_some();
        let total = if analyzed {
            plan.total_time()
        } else {
            plan.total_cost()
        }
        .unwrap_or(0.0);
        let rows = plan
            .flatten()
            .into_iter()
            .map(|(depth, node)| {
                let own = if analyzed {
                    node.exclusive_time()
                } else {
                    node.exclusive_cost()
                }
                .unwrap_or(0.0);
                PlanRow {
                    depth,
                    share: if total > 0.0 {
                        (own / total).clamp(0.0, 1.0) as f32
                    } else {
                        0.0
                    },
                    node: node.clone(),
                }
            })
            .collect();
        let column = |key: &str, label: SharedString, width: f32| {
            Column::new(SharedString::from(key.to_string()), label)
                .width(rems(width).to_pixels(rem))
        };
        let mut columns = vec![
            column("operation", t!("plan.operation").into(), 14.),
            column("target", t!("plan.target").into(), 11.),
            column("share", t!("plan.share").into(), 7.),
            column("cost", t!("plan.cost").into(), 6.).text_right(),
            column("rows", t!("plan.estimated_rows").into(), 6.).text_right(),
        ];
        if analyzed {
            columns.extend([
                column("actual_rows", t!("plan.actual_rows").into(), 6.).text_right(),
                column("time", t!("plan.time").into(), 6.).text_right(),
                column("loops", t!("plan.loops").into(), 5.).text_right(),
            ]);
        }
        Self { rows, columns }
    }
}

fn number(value: Option<f64>) -> SharedString {
    match value {
        Some(value) if value.fract() == 0.0 && value.abs() < 1e15 => {
            format::count(value as usize).into()
        }
        Some(value) => format!("{value:.2}").into(),
        None => SharedString::default(),
    }
}

impl TableDelegate for PlanGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let row = &self.rows[row_ix];
        let node = &row.node;
        let key = self.columns[col_ix].key.clone();
        let cell = div().w_full().truncate();
        match key.as_ref() {
            "operation" => cell
                .pl(rems(row.depth as f32 * 1.0))
                .child(SharedString::from(node.operation().to_string())),
            "target" => cell
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(node.target().unwrap_or_default().to_string())),
            "share" => {
                let color = if row.share > 0.5 {
                    cx.theme().danger
                } else if row.share > 0.2 {
                    cx.theme().warning
                } else {
                    cx.theme().primary
                };
                h_flex().w_full().gap_1().child(
                    div()
                        .h(px(6.))
                        .flex_1()
                        .rounded_full()
                        .bg(cx.theme().muted)
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(color)
                                .w(relative(row.share.max(0.01))),
                        ),
                ).child(
                    div()
                        .w(rems(2.5))
                        .text_right()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{:.0}%", row.share * 100.0)),
                )
            }
            "cost" => cell.text_right().child(number(node.total_cost())),
            "rows" => cell.text_right().child(number(node.estimated_rows())),
            "actual_rows" => cell
                .text_right()
                .when(
                    // Misestimates by an order of magnitude mislead the
                    // planner; flag them.
                    matches!(
                        (node.estimated_rows(), node.actual_rows()),
                        (Some(estimate), Some(actual))
                            if estimate > 0.0 && (actual / estimate > 10.0 || estimate / actual.max(1.0) > 10.0)
                    ),
                    |cell| cell.text_color(cx.theme().warning),
                )
                .child(number(node.actual_rows())),
            "time" => cell.text_right().child(number(node.actual_time())),
            "loops" => cell.text_right().child(number(node.loops())),
            _ => cell,
        }
        .into_any_element()
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        let node = &self.rows[row_ix].node;
        match self.columns[col_ix].key.as_ref() {
            "operation" => node.operation().to_string(),
            "target" => node.target().unwrap_or_default().to_string(),
            "share" => format!("{:.1}%", self.rows[row_ix].share * 100.0),
            "cost" => number(node.total_cost()).to_string(),
            "rows" => number(node.estimated_rows()).to_string(),
            "actual_rows" => number(node.actual_rows()).to_string(),
            "time" => number(node.actual_time()).to_string(),
            "loops" => number(node.loops()).to_string(),
            _ => String::new(),
        }
    }
}

/// A plan with the selected step's details beside it.
pub struct PlanView {
    table: Entity<TableState<PlanGrid>>,
    selected: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl PlanView {
    pub fn new(plan: &PlanNode, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let grid = PlanGrid::new(plan, window);
        let table = cx.new(|cx| TableState::new(grid, window, cx).sortable(false));
        let subscriptions = vec![cx.subscribe(&table, |this, _, event: &TableEvent, cx| {
            if let TableEvent::SelectRow(ix) = event {
                this.selected = Some(*ix);
                cx.notify();
            }
        })];
        Self {
            table,
            selected: Some(0),
            _subscriptions: subscriptions,
        }
    }

    fn render_details(&self, cx: &Context<Self>) -> impl IntoElement {
        let grid = self.table.read(cx).delegate();
        let node = self
            .selected
            .and_then(|ix| grid.rows.get(ix))
            .map(|row| &row.node);
        div()
            .id("plan-details")
            .flex()
            .flex_col()
            .w(rems(16.))
            .flex_none()
            .h_full()
            .overflow_y_scrollbar()
            .border_l_1()
            .border_color(cx.theme().border)
            .p_2()
            .gap_1()
            .text_xs()
            .when_some(node, |details, node| {
                details
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from(node.operation().to_string())),
                    )
                    .children(node.properties().iter().map(|(name, value)| {
                        v_flex()
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(name.to_string())),
                            )
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .child(SharedString::from(value.to_string())),
                            )
                    }))
            })
    }
}

impl Render for PlanView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(DataTable::new(&self.table).bordered(false).small()),
            )
            .child(self.render_details(cx))
    }
}
