//! The diagram of a schema: its tables and views as boxes, their foreign
//! keys as lines between the columns they join.

mod layout;

use std::sync::Arc;

use datakit_catalog::{Relation, Schema};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, PanelInfo, PanelState},
    h_flex, v_flex,
};
use gpui_kit::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, PathBuilder, Pixels, Point, Render, SharedString, Styled as _,
    Subscription, Window, canvas, div, point, prelude::FluentBuilder as _, px, rems,
};
use rust_i18n::t;

use crate::{
    datasource::{CatalogRequest, DataSource, DataSourceEvent},
    navigation::{Navigation, NavigationEvent},
    objects::{ObjectPath, ObjectRef},
};

use layout::{BOX_WIDTH, Shape};

/// How many columns a box lists before it says how many more there are.
const MAX_COLUMNS: usize = 24;
const HEADER_HEIGHT: f32 = 2.0;
const ROW_HEIGHT: f32 = 1.4;

struct TableBox {
    relation: Arc<str>,
    is_view: bool,
    /// Name, type, part of the key, part of a foreign key.
    columns: Vec<(SharedString, SharedString, bool, bool)>,
    hidden: usize,
    /// Top-left corner, in rems.
    x: f32,
    y: f32,
}

impl TableBox {
    fn height(&self) -> f32 {
        HEADER_HEIGHT + (self.columns.len() + usize::from(self.hidden > 0)) as f32 * ROW_HEIGHT
    }

    /// The vertical middle of `column`'s row, in rems from the top.
    fn row_middle(&self, column: Option<usize>) -> f32 {
        match column {
            Some(ix) if ix < self.columns.len() => {
                self.y + HEADER_HEIGHT + (ix as f32 + 0.5) * ROW_HEIGHT
            }
            _ => self.y + HEADER_HEIGHT / 2.0,
        }
    }
}

/// A reference from a column of one box to a column of another.
struct Edge {
    from: usize,
    from_row: Option<usize>,
    to: usize,
    to_row: Option<usize>,
}

enum Drag {
    /// Moving the whole diagram.
    Pan {
        start: Point<Pixels>,
        origin: (f32, f32),
    },
    /// Moving one box.
    Table {
        ix: usize,
        start: Point<Pixels>,
        origin: (f32, f32),
    },
}

/// The diagram window of one schema.
pub struct DiagramPanel {
    focus_handle: FocusHandle,
    data_source: Entity<DataSource>,
    schema: Arc<str>,
    boxes: Vec<TableBox>,
    edges: Vec<Edge>,
    /// Where the diagram's origin is in the window, in rems.
    pan: (f32, f32),
    drag: Option<Drag>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for DiagramPanel {}

impl DiagramPanel {
    pub const NAME: &str = "Diagram";

    pub fn new(data_source: Entity<DataSource>, schema: Arc<str>, cx: &mut Context<Self>) -> Self {
        let subscriptions =
            vec![
                cx.subscribe(&data_source, |this, _, event: &DataSourceEvent, cx| {
                    if matches!(event, DataSourceEvent::CatalogChanged) && this.boxes.is_empty() {
                        this.lay_out(cx);
                    }
                }),
            ];
        data_source.update(cx, |data_source, cx| {
            data_source.ensure(CatalogRequest::Objects(schema.clone()), cx)
        });
        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            data_source,
            schema,
            boxes: Vec::new(),
            edges: Vec::new(),
            pan: (1.0, 1.0),
            drag: None,
            _subscriptions: subscriptions,
        };
        panel.lay_out(cx);
        panel
    }

    fn schema<'a>(&self, cx: &'a App) -> Option<&'a Schema> {
        self.data_source.read(cx).loaded_schema(&self.schema)
    }

    /// Place every relation of the schema afresh.
    fn lay_out(&mut self, cx: &mut Context<Self>) {
        let Some(schema) = self.schema(cx).cloned() else {
            return;
        };
        let relations: Vec<&Relation> = schema.relations().unwrap_or_default().iter().collect();
        let index = |name: &str| relations.iter().position(|r| &*r.name() == name);
        let mut boxes: Vec<TableBox> = relations
            .iter()
            .map(|relation| {
                let key: Vec<Arc<str>> = relation.primary_key().iter().map(|c| c.name()).collect();
                let foreign: Vec<Arc<str>> = relation
                    .foreign_keys()
                    .flat_map(|(_, key)| key.columns().to_vec())
                    .collect();
                let columns: Vec<_> = relation
                    .columns()
                    .iter()
                    .take(MAX_COLUMNS)
                    .map(|column| {
                        (
                            SharedString::from(column.name().to_string()),
                            SharedString::from(column.data_type().to_string()),
                            key.contains(&column.name()),
                            foreign.contains(&column.name()),
                        )
                    })
                    .collect();
                TableBox {
                    relation: relation.name(),
                    is_view: relation.relation_type().is_view(),
                    hidden: relation.columns().len().saturating_sub(MAX_COLUMNS),
                    columns,
                    x: 0.0,
                    y: 0.0,
                }
            })
            .collect();
        let mut edges = Vec::new();
        let mut shapes = Vec::new();
        for (from, relation) in relations.iter().enumerate() {
            let mut references = Vec::new();
            for (_, key) in relation.foreign_keys() {
                if key.referenced_schema() != &*self.schema {
                    continue;
                }
                let Some(to) = index(key.referenced_relation()) else {
                    continue;
                };
                references.push(to);
                let row = |relation: &Relation, column: Option<&Arc<str>>| {
                    column.and_then(|column| {
                        relation
                            .columns()
                            .iter()
                            .position(|c| c.name() == *column)
                            .filter(|ix| *ix < MAX_COLUMNS)
                    })
                };
                edges.push(Edge {
                    from,
                    from_row: row(relation, key.columns().first()),
                    to,
                    to_row: row(relations[to], key.referenced_columns().first()),
                });
            }
            shapes.push(Shape {
                height: boxes[from].height(),
                references,
            });
        }
        for (table, (x, y)) in boxes.iter_mut().zip(layout::layout(&shapes)) {
            table.x = x;
            table.y = y;
        }
        self.boxes = boxes;
        self.edges = edges;
        cx.notify();
    }

    /// The schema as a Mermaid `erDiagram`, for documents and wikis.
    fn mermaid(&self) -> String {
        let mut text = String::from("erDiagram\n");
        for table in &self.boxes {
            text.push_str(&format!("    {} {{\n", table.relation));
            for (name, data_type, key, foreign) in &table.columns {
                let data_type: String = data_type
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect();
                let marker = match (key, foreign) {
                    (true, _) => " PK",
                    (false, true) => " FK",
                    _ => "",
                };
                text.push_str(&format!("        {data_type} {name}{marker}\n"));
            }
            text.push_str("    }\n");
        }
        for edge in &self.edges {
            text.push_str(&format!(
                "    {} ||--o{{ {} : \"\"\n",
                self.boxes[edge.to].relation, self.boxes[edge.from].relation
            ));
        }
        text
    }

    fn open(&self, ix: usize, cx: &mut App) {
        let object = ObjectRef::new(
            self.data_source.clone(),
            ObjectPath::relation(self.schema.clone(), self.boxes[ix].relation.clone()),
        );
        Navigation::request(NavigationEvent::Open(object), cx);
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drag = Some(Drag::Pan {
            start: event.position,
            origin: self.pan,
        });
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rem = f32::from(window.rem_size());
        let Some(drag) = &self.drag else {
            return;
        };
        if !event.dragging() {
            self.drag = None;
            return;
        }
        match drag {
            Drag::Pan { start, origin } => {
                let delta = event.position - *start;
                self.pan = (
                    origin.0 + f32::from(delta.x) / rem,
                    origin.1 + f32::from(delta.y) / rem,
                );
            }
            Drag::Table { ix, start, origin } => {
                let delta = event.position - *start;
                let ix = *ix;
                self.boxes[ix].x = origin.0 + f32::from(delta.x) / rem;
                self.boxes[ix].y = origin.1 + f32::from(delta.y) / rem;
            }
        }
        cx.notify();
    }

    fn render_box(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let table = &self.boxes[ix];
        let (x, y) = (table.x + self.pan.0, table.y + self.pan.1);
        v_flex()
            .id(("table", ix))
            .absolute()
            .left(rems(x))
            .top(rems(y))
            .w(rems(BOX_WIDTH))
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_sm()
            .overflow_hidden()
            .text_xs()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        this.open(ix, cx);
                        return;
                    }
                    let table = &this.boxes[ix];
                    this.drag = Some(Drag::Table {
                        ix,
                        start: event.position,
                        origin: (table.x, table.y),
                    });
                }),
            )
            .child(
                h_flex()
                    .h(rems(HEADER_HEIGHT))
                    .px_2()
                    .gap_1p5()
                    .bg(theme.muted)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Icon::new(if table.is_view {
                            IconName::Eye
                        } else {
                            IconName::Table
                        })
                        .xsmall()
                        .text_color(theme.primary),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .child(SharedString::from(table.relation.to_string())),
                    ),
            )
            .children(table.columns.iter().map(|(name, data_type, key, foreign)| {
                h_flex()
                    .h(rems(ROW_HEIGHT))
                    .px_2()
                    .gap_1()
                    .child(div().w_3().flex_none().map(|slot| {
                        if *key {
                            slot.child(Icon::new(IconName::Key).xsmall().text_color(theme.warning))
                        } else if *foreign {
                            slot.child(
                                Icon::new(IconName::Link)
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                            )
                        } else {
                            slot
                        }
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .child(
                        div()
                            .flex_none()
                            .max_w(rems(6.))
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(data_type.clone()),
                    )
            }))
            .when(table.hidden > 0, |list| {
                list.child(
                    div()
                        .h(rems(ROW_HEIGHT))
                        .px_2()
                        .text_color(theme.muted_foreground)
                        .child(t!("diagram.more_columns", count = table.hidden).to_string()),
                )
            })
    }
}

impl Focusable for DiagramPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for DiagramPanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }

    fn dump(&self, cx: &App) -> PanelState {
        let mut state = PanelState::new(Self::NAME);
        let saved = SavedDiagram {
            data_source: self.data_source.read(cx).profile().id().clone(),
            schema: self.schema.to_string(),
        };
        state.info = PanelInfo::panel(serde_json::to_value(saved).unwrap_or_default());
        state
    }
}

/// A diagram's identity in a saved layout.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedDiagram {
    data_source: datakit_driver::DataSourceId,
    schema: String,
}

impl DiagramPanel {
    /// The data source and schema of the diagram a saved layout describes.
    pub fn saved(state: &PanelState) -> Option<(datakit_driver::DataSourceId, Arc<str>)> {
        let PanelInfo::Panel(value) = &state.info else {
            return None;
        };
        let saved: SavedDiagram = serde_json::from_value(value.clone()).ok()?;
        Some((saved.data_source, saved.schema.into()))
    }
}

impl Panel for DiagramPanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .child(
                Icon::new(IconName::Workflow)
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(SharedString::from(self.schema.to_string()))
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for DiagramPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let boxes: Vec<gpui_kit::AnyElement> = (0..self.boxes.len())
            .map(|ix| self.render_box(ix, cx).into_any_element())
            .collect();
        let theme = cx.theme();
        let rem = f32::from(window.rem_size());
        // Each line goes from the referring column to the referred one.
        let lines: Vec<(Point<f32>, Point<f32>)> = self
            .edges
            .iter()
            .map(|edge| {
                let from = &self.boxes[edge.from];
                let to = &self.boxes[edge.to];
                let rightwards = to.x > from.x;
                let from_x = if rightwards {
                    from.x + BOX_WIDTH
                } else {
                    from.x
                };
                let to_x = if rightwards { to.x } else { to.x + BOX_WIDTH };
                (
                    point(
                        from_x + self.pan.0,
                        from.row_middle(edge.from_row) + self.pan.1,
                    ),
                    point(to_x + self.pan.0, to.row_middle(edge.to_row) + self.pan.1),
                )
            })
            .collect();
        let line_color = theme.muted_foreground.opacity(0.7);
        let edges = canvas(
            move |_, _, _| (),
            move |bounds, _, window, _| {
                for (from, to) in &lines {
                    let at = |x: f32, y: f32| bounds.origin + point(px(x * rem), px(y * rem));
                    let middle = (from.x + to.x) / 2.0;
                    let mut path = PathBuilder::stroke(px(1.5));
                    path.move_to(at(from.x, from.y));
                    path.line_to(at(middle, from.y));
                    path.line_to(at(middle, to.y));
                    path.line_to(at(to.x, to.y));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, line_color);
                    }
                    // A short bar marks the referred end.
                    let side = if to.x > middle { -0.4 } else { 0.4 };
                    let mut bar = PathBuilder::stroke(px(2.));
                    bar.move_to(at(to.x + side, to.y - 0.35));
                    bar.line_to(at(to.x + side, to.y + 0.35));
                    if let Ok(bar) = bar.build() {
                        window.paint_path(bar, line_color);
                    }
                }
            },
        )
        .absolute()
        .size_full();
        let empty = self.boxes.is_empty();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("diagram-relayout")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(t!("diagram.relayout").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pan = (1.0, 1.0);
                                this.lay_out(cx);
                            })),
                    )
                    .child(
                        Button::new("diagram-mermaid")
                            .ghost()
                            .small()
                            .icon(IconName::Copy)
                            .label(t!("diagram.copy_mermaid").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(this.mermaid()));
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("diagram.hint").to_string()),
                    ),
            )
            .child(
                div()
                    .id("diagram-canvas")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(theme.muted.opacity(0.3))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.drag = None;
                            cx.notify();
                        }),
                    )
                    .child(edges)
                    .children(boxes)
                    .when(empty, |canvas| {
                        canvas.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(t!("explorer.loading").to_string()),
                        )
                    }),
            )
    }
}
