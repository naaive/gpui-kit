//! Modify table and New table: a table's columns, indexes and foreign keys
//! as editable rows, with the statements that would make it so.

mod model;

use std::sync::Arc;

use datakit_catalog::{ReferentialAction, Relation};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Editor, EditorState, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    scroll::ScrollableElement as _,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px, rems,
};
use rust_i18n::t;

use crate::{
    datasource::{CatalogRequest, DataSource, describe_error},
    objects,
};

use model::{ColumnDraft, DraftProblem, ForeignKeyDraft, IndexDraft, TableDraft};

struct ColumnRow {
    original: Option<Arc<str>>,
    name: Entity<InputState>,
    data_type: Entity<InputState>,
    default: Entity<InputState>,
    comment: Entity<InputState>,
    not_null: bool,
    primary_key: bool,
    auto_increment: bool,
    generated: bool,
}

struct IndexRow {
    original: Option<datakit_catalog::Index>,
    name: Entity<InputState>,
    columns: Entity<InputState>,
    unique: bool,
}

struct ForeignKeyRow {
    original: Option<datakit_catalog::Constraint>,
    name: Entity<InputState>,
    columns: Entity<InputState>,
    referenced: Entity<InputState>,
    referenced_columns: Entity<InputState>,
    on_delete: ReferentialAction,
    on_update: ReferentialAction,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DesignerTab {
    Columns,
    Indexes,
    ForeignKeys,
}

/// The table designer, shown in a dialog.
pub struct TableDesigner {
    data_source: Entity<DataSource>,
    schema: Arc<str>,
    original: Option<Relation>,
    name: Entity<InputState>,
    comment: Entity<InputState>,
    columns: Vec<ColumnRow>,
    indexes: Vec<IndexRow>,
    foreign_keys: Vec<ForeignKeyRow>,
    tab: DesignerTab,
    preview: Entity<EditorState>,
    problem: Option<SharedString>,
    /// Whether there are statements to run.
    ready: bool,
    executing: Option<Task<()>>,
    subscriptions: Vec<Subscription>,
}

impl TableDesigner {
    /// Open the designer for `relation` of `schema`, or for a new table.
    pub fn open(
        data_source: &Entity<DataSource>,
        schema: Arc<str>,
        relation: Option<Arc<str>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let original = relation.as_ref().and_then(|name| {
            data_source
                .read(cx)
                .catalog()
                .schema(&schema)?
                .relation(name)
                .cloned()
        });
        if relation.is_some() && original.is_none() {
            // Not loaded yet: load it, and ask again.
            data_source.update(cx, |data_source, cx| {
                data_source.ensure(CatalogRequest::Objects(schema.clone()), cx)
            });
            window.push_notification(Notification::info(t!("designer.loading").to_string()), cx);
            return;
        }
        let title: SharedString = match &original {
            Some(relation) => t!("designer.modify_title", name = relation.name()).into(),
            None => t!("designer.new_title", schema = schema).into(),
        };
        let data_source = data_source.clone();
        let designer = cx.new(|cx| Self::new(data_source, schema, original, window, cx));
        window.open_dialog(cx, {
            let designer = designer.clone();
            move |dialog, _, cx| {
                let executing = designer.read(cx).executing.is_some();
                dialog
                    .title(title.clone())
                    .w(px(920.))
                    .child(designer.clone())
                    .footer(
                        DialogFooter::new().child(
                            h_flex()
                                .gap_2()
                                .child(
                                    DialogClose::new().child(
                                        Button::new("cancel")
                                            .outline()
                                            .label(t!("common.cancel").to_string()),
                                    ),
                                )
                                .child(
                                    DialogAction::new().child(
                                        Button::new("execute")
                                            .primary()
                                            .loading(executing)
                                            .label(t!("designer.execute").to_string()),
                                    ),
                                ),
                        ),
                    )
                    .on_ok({
                        let designer = designer.clone();
                        move |_, window, cx| {
                            designer.update(cx, |designer, cx| designer.execute(window, cx));
                            false
                        }
                    })
            }
        });
        let name = designer.read(cx).name.clone();
        name.update(cx, |input, cx| input.focus(window, cx));
    }

    fn new(
        data_source: Entity<DataSource>,
        schema: Arc<str>,
        original: Option<Relation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id_type = data_source
            .read(cx)
            .dialect()
            .data_types()
            .iter()
            .find(|ty| ty.contains("bigint") || ty.contains("int"))
            .copied()
            .unwrap_or("integer");
        let draft = match &original {
            Some(relation) => TableDraft::from_relation(relation),
            None => TableDraft::new_table(id_type),
        };
        let preview = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .line_number(false)
        });
        let mut designer = Self {
            data_source,
            schema,
            name: Self::input(&draft.name, "", window, cx),
            comment: Self::input(&draft.comment, "", window, cx),
            original,
            columns: Vec::new(),
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            tab: DesignerTab::Columns,
            preview,
            problem: None,
            ready: false,
            executing: None,
            subscriptions: Vec::new(),
        };
        for input in [designer.name.clone(), designer.comment.clone()] {
            designer.watch(&input, window, cx);
        }
        for column in &draft.columns {
            designer.push_column(column, window, cx);
        }
        for index in &draft.indexes {
            designer.push_index(index, window, cx);
        }
        for key in &draft.foreign_keys {
            designer.push_foreign_key(key, window, cx);
        }
        designer.update_preview(window, cx);
        designer
    }

    fn input(
        value: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let value = SharedString::from(value.to_string());
        let placeholder = SharedString::from(placeholder.to_string());
        cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(value)
        })
    }

    /// Redo the preview whenever `input` changes.
    fn watch(&mut self, input: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) {
        self.subscriptions.push(cx.subscribe_in(
            input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.update_preview(window, cx);
                }
            },
        ));
    }

    fn push_column(&mut self, draft: &ColumnDraft, window: &mut Window, cx: &mut Context<Self>) {
        let row = ColumnRow {
            original: draft.original.clone(),
            name: Self::input(&draft.name, "name", window, cx),
            data_type: Self::input(&draft.data_type, "type", window, cx),
            default: Self::input(&draft.default, "", window, cx),
            comment: Self::input(&draft.comment, "", window, cx),
            not_null: draft.not_null,
            primary_key: draft.primary_key,
            auto_increment: draft.auto_increment,
            generated: draft.generated,
        };
        for input in [&row.name, &row.data_type, &row.default, &row.comment] {
            let input = input.clone();
            self.watch(&input, window, cx);
        }
        self.columns.push(row);
    }

    fn push_index(&mut self, draft: &IndexDraft, window: &mut Window, cx: &mut Context<Self>) {
        let row = IndexRow {
            original: draft.original.clone(),
            name: Self::input(&draft.name, &t!("designer.generated_name"), window, cx),
            columns: Self::input(&draft.columns, "a, b", window, cx),
            unique: draft.unique,
        };
        for input in [&row.name, &row.columns] {
            let input = input.clone();
            self.watch(&input, window, cx);
        }
        self.indexes.push(row);
    }

    fn push_foreign_key(
        &mut self,
        draft: &ForeignKeyDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = ForeignKeyRow {
            original: draft.original.clone(),
            name: Self::input(&draft.name, &t!("designer.generated_name"), window, cx),
            columns: Self::input(&draft.columns, "customer_id", window, cx),
            referenced: Self::input(&draft.referenced, "schema.table", window, cx),
            referenced_columns: Self::input(&draft.referenced_columns, "id", window, cx),
            on_delete: draft.on_delete,
            on_update: draft.on_update,
        };
        for input in [
            &row.name,
            &row.columns,
            &row.referenced,
            &row.referenced_columns,
        ] {
            let input = input.clone();
            self.watch(&input, window, cx);
        }
        self.foreign_keys.push(row);
    }

    fn draft(&self, cx: &App) -> TableDraft {
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        TableDraft {
            original: self.original.clone(),
            name: text(&self.name),
            comment: text(&self.comment),
            columns: self
                .columns
                .iter()
                .map(|row| ColumnDraft {
                    original: row.original.clone(),
                    name: text(&row.name),
                    data_type: text(&row.data_type),
                    not_null: row.not_null,
                    default: text(&row.default),
                    comment: text(&row.comment),
                    primary_key: row.primary_key,
                    auto_increment: row.auto_increment,
                    generated: row.generated,
                })
                .collect(),
            indexes: self
                .indexes
                .iter()
                .map(|row| IndexDraft {
                    original: row.original.clone(),
                    name: text(&row.name),
                    columns: text(&row.columns),
                    unique: row.unique,
                })
                .collect(),
            foreign_keys: self
                .foreign_keys
                .iter()
                .map(|row| ForeignKeyDraft {
                    original: row.original.clone(),
                    name: text(&row.name),
                    columns: text(&row.columns),
                    referenced: text(&row.referenced),
                    referenced_columns: text(&row.referenced_columns),
                    on_delete: row.on_delete,
                    on_update: row.on_update,
                })
                .collect(),
        }
    }

    fn statements(&self, cx: &App) -> Result<Vec<String>, DraftProblem> {
        let dialect = self.data_source.read(cx).dialect();
        self.draft(cx).statements(&*dialect, &self.schema)
    }

    fn update_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, problem) = match self.statements(cx) {
            Ok(statements) => (objects::script(&statements), None),
            Err(problem) => (String::new(), Some(problem_text(&problem))),
        };
        let ready = !text.trim().is_empty();
        if ready != self.ready {
            // The dialog's footer reads it; the dialog draws apart from us.
            window.refresh();
        }
        self.ready = ready;
        self.problem = problem;
        self.preview.update(cx, |editor, cx| {
            editor.set_value(text, window, cx);
            editor.set_readonly(true, cx);
        });
        cx.notify();
    }

    fn execute(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.executing.is_some() {
            return;
        }
        let statements = match self.statements(cx) {
            Ok(statements) if !statements.is_empty() => statements,
            _ => return,
        };
        let task = self.data_source.read(cx).run_statements(statements, cx);
        let data_source = self.data_source.clone();
        let schema = self.schema.clone();
        window.refresh();
        self.executing = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.executing = None;
                match result {
                    Ok(()) => {
                        data_source.update(cx, |data_source, cx| {
                            data_source.request(CatalogRequest::Objects(schema.clone()), cx)
                        });
                        window.close_dialog(cx);
                    }
                    Err(error) => this.problem = Some(describe_error(&error)),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn render_columns(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let header = |text: SharedString, width: Option<f32>| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .map(|cell| match width {
                    Some(width) => cell.w(rems(width)).flex_none(),
                    None => cell.flex_1(),
                })
                .child(text)
        };
        let types: Vec<&'static str> = self.data_source.read(cx).dialect().data_types().to_vec();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(header(t!("designer.column").into(), Some(9.)))
                    .child(header(t!("designer.type").into(), Some(11.)))
                    .child(header("NOT NULL".into(), Some(4.5)))
                    .child(header("PK".into(), Some(2.5)))
                    .child(header(t!("designer.default").into(), Some(8.)))
                    .child(header(t!("designer.comment").into(), None))
                    .child(div().w(rems(1.75)).flex_none()),
            )
            .children(self.columns.iter().enumerate().map(|(ix, row)| {
                let types = types.clone();
                let data_type = row.data_type.clone();
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .w(rems(9.))
                            .flex_none()
                            .child(Input::new(&row.name).small()),
                    )
                    .child(
                        h_flex()
                            .w(rems(11.))
                            .flex_none()
                            .child(Input::new(&row.data_type).small())
                            .child(
                                Button::new(("types", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ChevronDown)
                                    .dropdown_menu(move |menu, _, _| {
                                        types.iter().fold(menu, |menu, ty| {
                                            let data_type = data_type.clone();
                                            let ty = *ty;
                                            menu.item(PopupMenuItem::new(ty).on_click(
                                                move |_, window, cx| {
                                                    data_type.update(cx, |input, cx| {
                                                        input.set_value(ty, window, cx);
                                                        cx.emit(InputEvent::Change);
                                                    })
                                                },
                                            ))
                                        })
                                    }),
                            ),
                    )
                    .child(
                        div().w(rems(4.5)).flex_none().child(
                            Checkbox::new(("not-null", ix))
                                .checked(row.not_null || row.primary_key)
                                .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                                    this.columns[ix].not_null = *checked;
                                    this.update_preview(window, cx);
                                })),
                        ),
                    )
                    .child(
                        div().w(rems(2.5)).flex_none().child(
                            Checkbox::new(("primary-key", ix))
                                .checked(row.primary_key)
                                .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                                    this.columns[ix].primary_key = *checked;
                                    this.update_preview(window, cx);
                                })),
                        ),
                    )
                    .child(
                        div()
                            .w(rems(8.))
                            .flex_none()
                            .child(Input::new(&row.default).small()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&row.comment).small()),
                    )
                    .child(
                        Button::new(("remove-column", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Minus)
                            .tooltip(t!("designer.remove_column").to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.columns.remove(ix);
                                this.update_preview(window, cx);
                            })),
                    )
            }))
            .child(
                h_flex().child(
                    Button::new("add-column")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label(t!("designer.add_column").to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            let draft = ColumnDraft {
                                data_type: "text".into(),
                                ..Default::default()
                            };
                            this.push_column(&draft, window, cx);
                            if let Some(row) = this.columns.last() {
                                row.name.update(cx, |input, cx| input.focus(window, cx));
                            }
                            this.update_preview(window, cx);
                        })),
                ),
            )
            .into_any_element()
    }

    fn render_indexes(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let header = |text: SharedString, width: Option<f32>| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .map(|cell| match width {
                    Some(width) => cell.w(rems(width)).flex_none(),
                    None => cell.flex_1(),
                })
                .child(text)
        };
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(header(t!("designer.name").into(), Some(14.)))
                    .child(header(t!("designer.columns").into(), None))
                    .child(header("UNIQUE".into(), Some(4.5)))
                    .child(div().w(rems(1.75)).flex_none()),
            )
            .children(self.indexes.iter().enumerate().map(|(ix, row)| {
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .w(rems(14.))
                            .flex_none()
                            .child(Input::new(&row.name).small()),
                    )
                    .child(div().flex_1().child(Input::new(&row.columns).small()))
                    .child(
                        div().w(rems(4.5)).flex_none().child(
                            Checkbox::new(("unique", ix)).checked(row.unique).on_click(
                                cx.listener(move |this, checked: &bool, window, cx| {
                                    this.indexes[ix].unique = *checked;
                                    this.update_preview(window, cx);
                                }),
                            ),
                        ),
                    )
                    .child(
                        Button::new(("remove-index", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Minus)
                            .tooltip(t!("designer.remove_index").to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.indexes.remove(ix);
                                this.update_preview(window, cx);
                            })),
                    )
            }))
            .child(
                h_flex().child(
                    Button::new("add-index")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label(t!("designer.add_index").to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.push_index(&IndexDraft::default(), window, cx);
                            this.update_preview(window, cx);
                        })),
                ),
            )
            .into_any_element()
    }

    fn render_foreign_keys(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let header = |text: SharedString, width: Option<f32>| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .map(|cell| match width {
                    Some(width) => cell.w(rems(width)).flex_none(),
                    None => cell.flex_1(),
                })
                .child(text)
        };
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(header(t!("designer.name").into(), Some(11.)))
                    .child(header(t!("designer.columns").into(), Some(9.)))
                    .child(header(t!("designer.references").into(), None))
                    .child(header(t!("designer.referenced_columns").into(), Some(8.)))
                    .child(header("ON DELETE".into(), Some(8.)))
                    .child(div().w(rems(1.75)).flex_none()),
            )
            .children(self.foreign_keys.iter().enumerate().map(|(ix, row)| {
                let view = cx.entity().downgrade();
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .w(rems(11.))
                            .flex_none()
                            .child(Input::new(&row.name).small()),
                    )
                    .child(
                        div()
                            .w(rems(9.))
                            .flex_none()
                            .child(Input::new(&row.columns).small()),
                    )
                    .child(div().flex_1().child(Input::new(&row.referenced).small()))
                    .child(
                        div()
                            .w(rems(8.))
                            .flex_none()
                            .child(Input::new(&row.referenced_columns).small()),
                    )
                    .child(
                        div().w(rems(8.)).flex_none().child(
                            Button::new(("on-delete", ix))
                                .outline()
                                .small()
                                .label(row.on_delete.sql())
                                .dropdown_menu(move |menu, _, _| {
                                    ReferentialAction::ALL.iter().fold(menu, |menu, action| {
                                        let action = *action;
                                        let view = view.clone();
                                        menu.item(PopupMenuItem::new(action.sql()).on_click(
                                            move |_, window, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.foreign_keys[ix].on_delete = action;
                                                    this.update_preview(window, cx);
                                                });
                                            },
                                        ))
                                    })
                                }),
                        ),
                    )
                    .child(
                        Button::new(("remove-key", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Minus)
                            .tooltip(t!("designer.remove_foreign_key").to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.foreign_keys.remove(ix);
                                this.update_preview(window, cx);
                            })),
                    )
            }))
            .child(
                h_flex().child(
                    Button::new("add-key")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label(t!("designer.add_foreign_key").to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.push_foreign_key(&ForeignKeyDraft::default(), window, cx);
                            this.update_preview(window, cx);
                        })),
                ),
            )
            .into_any_element()
    }
}

fn problem_text(problem: &DraftProblem) -> SharedString {
    match problem {
        DraftProblem::NoName => t!("designer.problem.no_name"),
        DraftProblem::NoColumns => t!("designer.problem.no_columns"),
        DraftProblem::ColumnWithoutName(position) => {
            t!("designer.problem.column_without_name", position = position)
        }
        DraftProblem::ColumnWithoutType(name) => {
            t!("designer.problem.column_without_type", name = name)
        }
        DraftProblem::DuplicateColumn(name) => t!("designer.problem.duplicate_column", name = name),
        DraftProblem::IndexWithoutColumns(name) => {
            t!("designer.problem.index_without_columns", name = name)
        }
        DraftProblem::ForeignKeyIncomplete(name) => {
            t!("designer.problem.foreign_key_incomplete", name = name)
        }
    }
    .into()
}

impl Render for TableDesigner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let selected = match self.tab {
            DesignerTab::Columns => 0,
            DesignerTab::Indexes => 1,
            DesignerTab::ForeignKeys => 2,
        };
        let content = match self.tab {
            DesignerTab::Columns => self.render_columns(cx),
            DesignerTab::Indexes => self.render_indexes(cx),
            DesignerTab::ForeignKeys => self.render_foreign_keys(cx),
        };
        let theme = cx.theme();
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .w(rems(16.))
                            .gap_1()
                            .text_sm()
                            .child(t!("designer.table_name").to_string())
                            .child(Input::new(&self.name)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .text_sm()
                            .child(t!("designer.comment").to_string())
                            .child(Input::new(&self.comment)),
                    ),
            )
            .child(
                TabBar::new("designer-tabs")
                    .small()
                    .child(
                        Tab::new().label(
                            t!("designer.columns_tab", count = self.columns.len()).to_string(),
                        ),
                    )
                    .child(
                        Tab::new().label(
                            t!("designer.indexes_tab", count = self.indexes.len()).to_string(),
                        ),
                    )
                    .child(
                        Tab::new().label(
                            t!("designer.foreign_keys_tab", count = self.foreign_keys.len())
                                .to_string(),
                        ),
                    )
                    .selected_index(selected)
                    .on_click(move |ix: &usize, _, cx| {
                        let ix = *ix;
                        let _ = view.update(cx, |this, cx| {
                            this.tab = match ix {
                                0 => DesignerTab::Columns,
                                1 => DesignerTab::Indexes,
                                _ => DesignerTab::ForeignKeys,
                            };
                            cx.notify();
                        });
                    }),
            )
            .child(
                div()
                    .id("designer-rows")
                    .h(px(260.))
                    .overflow_y_scrollbar()
                    .child(content),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("designer.preview").to_string()),
                    )
                    .child(
                        div()
                            .h(px(140.))
                            .border_1()
                            .border_color(theme.border)
                            .rounded(theme.radius)
                            .child(
                                Editor::new(&self.preview)
                                    .bordered(false)
                                    .h_full()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_size(theme.mono_font_size),
                            ),
                    )
                    .map(|preview| match self.problem.clone() {
                        Some(problem) => {
                            preview.child(div().text_sm().text_color(theme.danger).child(problem))
                        }
                        None if !self.ready => preview.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(t!("designer.no_changes").to_string()),
                        ),
                        None => preview,
                    }),
            )
    }
}
