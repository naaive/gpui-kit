//! Schema comparison: what has to change for one schema to look like
//! another, as a tree of differences and as the script that does it.
//!
//! The two schemas can be in different data sources, even of different
//! databases; the script is written in the target's dialect.

use std::{collections::HashMap, rc::Rc, sync::Arc};

use datakit_catalog::{SchemaDiff, diff_schemas};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent},
    h_flex,
    input::{Editor, EditorState},
    list::ListItem,
    resizable::{h_resizable, resizable_panel},
    select::{Select, SelectEvent, SelectItem, SelectState},
    tree::{TreeItem, TreeState, tree},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
    rems,
};
use rust_i18n::t;

use crate::{
    datasource::{CatalogRequest, DataSource, DataSourceEvent, DataSources},
    navigation::{Navigation, NavigationEvent},
    objects::{self, ObjectPath, ObjectRef},
};

/// One schema of one data source.
#[derive(Clone)]
struct SchemaChoice {
    data_source: Entity<DataSource>,
    schema: Arc<str>,
    title: SharedString,
    key: SharedString,
}

impl SelectItem for SchemaChoice {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &SharedString {
        &self.key
    }
}

/// How a difference changes the target.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    Added,
    Removed,
    Changed,
}

/// The schema comparison window.
pub struct ComparePanel {
    focus_handle: FocusHandle,
    source: Entity<SelectState<Vec<SchemaChoice>>>,
    target: Entity<SelectState<Vec<SchemaChoice>>>,
    choices: Vec<SchemaChoice>,
    tree: Entity<TreeState>,
    changes: Rc<HashMap<SharedString, Change>>,
    script: Entity<EditorState>,
    diff: Option<SchemaDiff>,
    /// Waiting for a schema to be read before comparing.
    waiting: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for ComparePanel {}

impl ComparePanel {
    pub const NAME: &str = "Compare";

    /// A comparison with `source` already chosen.
    pub fn new(
        source: Option<(Entity<DataSource>, Arc<str>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let choices = schema_choices(cx);
        let source_ix = source.as_ref().and_then(|(data_source, schema)| {
            choices
                .iter()
                .position(|choice| &choice.data_source == data_source && &choice.schema == schema)
        });
        let source_state = cx
            .new(|cx| SelectState::new(choices.clone(), source_ix.map(IndexPath::new), window, cx));
        let target_state = cx.new(|cx| SelectState::new(choices.clone(), None, window, cx));
        let script = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .line_number(true)
        });
        let mut subscriptions = vec![
            cx.subscribe_in(
                &source_state,
                window,
                |this, _, _: &SelectEvent<Vec<SchemaChoice>>, window, cx| this.compare(window, cx),
            ),
            cx.subscribe_in(
                &target_state,
                window,
                |this, _, _: &SelectEvent<Vec<SchemaChoice>>, window, cx| this.compare(window, cx),
            ),
        ];
        for data_source in DataSources::global(cx).read(cx).items().to_vec() {
            subscriptions.push(cx.subscribe_in(
                &data_source,
                window,
                |this, _, event: &DataSourceEvent, window, cx| {
                    if matches!(event, DataSourceEvent::CatalogChanged) && this.waiting {
                        this.compare(window, cx);
                    }
                },
            ));
        }
        Self {
            focus_handle: cx.focus_handle(),
            source: source_state,
            target: target_state,
            choices,
            tree: cx.new(|cx| TreeState::new(cx)),
            changes: Rc::default(),
            script,
            diff: None,
            waiting: false,
            _subscriptions: subscriptions,
        }
    }

    fn chosen(
        &self,
        state: &Entity<SelectState<Vec<SchemaChoice>>>,
        cx: &App,
    ) -> Option<SchemaChoice> {
        let key = state.read(cx).selected_value()?.clone();
        self.choices
            .iter()
            .find(|choice| choice.key == key)
            .cloned()
    }

    /// Compare the chosen schemas, reading them first if they are not.
    fn compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(source), Some(target)) =
            (self.chosen(&self.source, cx), self.chosen(&self.target, cx))
        else {
            return;
        };
        let mut missing = false;
        for choice in [&source, &target] {
            if choice
                .data_source
                .read(cx)
                .loaded_schema(&choice.schema)
                .is_none()
            {
                missing = true;
                choice.data_source.update(cx, |data_source, cx| {
                    data_source.ensure(CatalogRequest::Objects(choice.schema.clone()), cx)
                });
            }
        }
        self.waiting = missing;
        if missing {
            cx.notify();
            return;
        }
        let source_schema = source
            .data_source
            .read(cx)
            .loaded_schema(&source.schema)
            .cloned();
        let target_schema = target
            .data_source
            .read(cx)
            .loaded_schema(&target.schema)
            .cloned();
        let (Some(source_schema), Some(target_schema)) = (source_schema, target_schema) else {
            return;
        };
        let diff = diff_schemas(&target_schema, &source_schema);
        let dialect = target.data_source.read(cx).dialect();
        let script = objects::script(&dialect.migrate(&target.schema, &diff));
        let (items, changes) = diff_tree(&diff);
        self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
        self.changes = Rc::new(changes);
        self.script.update(cx, |editor, cx| {
            editor.set_value(
                if diff.is_empty() {
                    String::new()
                } else {
                    script
                },
                window,
                cx,
            );
            editor.set_readonly(true, cx);
        });
        self.diff = Some(diff);
        cx.notify();
    }

    fn open_in_console(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.chosen(&self.target, cx) else {
            return;
        };
        let sql = self.script.read(cx).value().to_string();
        if sql.trim().is_empty() {
            return;
        }
        let object = ObjectRef::new(
            target.data_source.clone(),
            ObjectPath::Schema {
                schema: target.schema.clone(),
            },
        );
        Navigation::request(NavigationEvent::OpenConsole { object, sql }, cx);
    }
}

fn schema_choices(cx: &App) -> Vec<SchemaChoice> {
    let mut choices = Vec::new();
    for data_source in DataSources::global(cx).read(cx).items() {
        let source = data_source.read(cx);
        for schema in source.catalog().schemas() {
            if schema.is_system() {
                continue;
            }
            choices.push(SchemaChoice {
                data_source: data_source.clone(),
                schema: schema.name(),
                title: format!("{} · {}", source.name(), schema.name()).into(),
                key: format!("{}/{}", source.profile().id(), schema.name()).into(),
            });
        }
    }
    choices
}

/// The differences as tree rows, and what each row's change is.
fn diff_tree(diff: &SchemaDiff) -> (Vec<TreeItem>, HashMap<SharedString, Change>) {
    let mut changes = HashMap::new();
    let mut leaf = |id: String, label: String, change: Change| {
        changes.insert(SharedString::from(id.clone()), change);
        TreeItem::new(id, label)
    };
    let mut items = Vec::new();
    for relation in diff.added_relations() {
        items.push(leaf(
            format!("+r/{}", relation.name()),
            relation.name().to_string(),
            Change::Added,
        ));
    }
    for relation in diff.removed_relations() {
        items.push(leaf(
            format!("-r/{}", relation.name()),
            relation.name().to_string(),
            Change::Removed,
        ));
    }
    for change in diff.changed_relations() {
        let name = change.source().name();
        let mut children = Vec::new();
        for column in change.added_columns() {
            children.push(leaf(
                format!("~r/{name}/+c/{}", column.name()),
                format!("{} {}", column.name(), column.data_type()),
                Change::Added,
            ));
        }
        for column in change.removed_columns() {
            children.push(leaf(
                format!("~r/{name}/-c/{}", column.name()),
                format!("{} {}", column.name(), column.data_type()),
                Change::Removed,
            ));
        }
        for (old, new) in change.changed_columns() {
            let mut label = new.name().to_string();
            if old.data_type() != new.data_type() {
                label.push_str(&format!(" {} → {}", old.data_type(), new.data_type()));
            }
            if old.is_nullable() != new.is_nullable() {
                label.push_str(if new.is_nullable() {
                    " · NULL"
                } else {
                    " · NOT NULL"
                });
            }
            if old.default() != new.default() {
                label.push_str(&format!(" · = {}", new.default().unwrap_or("–")));
            }
            children.push(leaf(
                format!("~r/{name}/~c/{}", new.name()),
                label,
                Change::Changed,
            ));
        }
        for index in change.added_indexes() {
            children.push(leaf(
                format!("~r/{name}/+i/{}", index.name()),
                index.name().to_string(),
                Change::Added,
            ));
        }
        for index in change.removed_indexes() {
            children.push(leaf(
                format!("~r/{name}/-i/{}", index.name()),
                index.name().to_string(),
                Change::Removed,
            ));
        }
        for constraint in change.added_constraints() {
            children.push(leaf(
                format!("~r/{name}/+k/{}", constraint.name()),
                constraint.name().to_string(),
                Change::Added,
            ));
        }
        for constraint in change.removed_constraints() {
            children.push(leaf(
                format!("~r/{name}/-k/{}", constraint.name()),
                constraint.name().to_string(),
                Change::Removed,
            ));
        }
        for trigger in change.added_triggers() {
            children.push(leaf(
                format!("~r/{name}/+t/{}", trigger.name()),
                trigger.name().to_string(),
                Change::Added,
            ));
        }
        for trigger in change.removed_triggers() {
            children.push(leaf(
                format!("~r/{name}/-t/{}", trigger.name()),
                trigger.name().to_string(),
                Change::Removed,
            ));
        }
        if change.is_definition_changed() {
            children.push(leaf(
                format!("~r/{name}/definition"),
                t!("compare.definition").to_string(),
                Change::Changed,
            ));
        }
        if change.is_comment_changed() {
            children.push(leaf(
                format!("~r/{name}/comment"),
                t!("compare.comment").to_string(),
                Change::Changed,
            ));
        }
        items.push(
            leaf(format!("~r/{name}"), name.to_string(), Change::Changed)
                .expanded(true)
                .children(children),
        );
    }
    for routine in diff.added_routines() {
        items.push(leaf(
            format!("+f/{}", routine.signature()),
            routine.signature(),
            Change::Added,
        ));
    }
    for routine in diff.removed_routines() {
        items.push(leaf(
            format!("-f/{}", routine.signature()),
            routine.signature(),
            Change::Removed,
        ));
    }
    for (_, routine) in diff.changed_routines() {
        items.push(leaf(
            format!("~f/{}", routine.signature()),
            routine.signature(),
            Change::Changed,
        ));
    }
    for sequence in diff.added_sequences() {
        items.push(leaf(
            format!("+q/{}", sequence.name()),
            sequence.name().to_string(),
            Change::Added,
        ));
    }
    for sequence in diff.removed_sequences() {
        items.push(leaf(
            format!("-q/{}", sequence.name()),
            sequence.name().to_string(),
            Change::Removed,
        ));
    }
    (items, changes)
}

impl Focusable for ComparePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for ComparePanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }
}

impl Panel for ComparePanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .child(
                Icon::new(IconName::GitCompare)
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(t!("compare.title").to_string())
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for ComparePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let changes = self.changes.clone();
        let has_script = !self.script.read(cx).value().trim().is_empty();
        let body: gpui_kit::AnyElement = match &self.diff {
            _ if self.waiting => div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t!("explorer.loading").to_string())
                .into_any_element(),
            None => div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t!("compare.choose").to_string())
                .into_any_element(),
            Some(diff) if diff.is_empty() => div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t!("compare.identical").to_string())
                .into_any_element(),
            Some(_) => h_resizable("compare-split")
                .child(
                    resizable_panel()
                        .size(rems(24.).to_pixels(window.rem_size()))
                        .child(div().size_full().p_1().child(tree(
                            &self.tree,
                            move |ix, entry, selected, _, cx| {
                                let item = entry.item();
                                let theme = cx.theme();
                                let (icon, color) = match changes.get(&item.id) {
                                    Some(Change::Added) => (IconName::Plus, theme.success),
                                    Some(Change::Removed) => (IconName::Minus, theme.danger),
                                    _ => (IconName::Diff, theme.warning),
                                };
                                ListItem::new(ix)
                                    .selected(selected)
                                    .py_0p5()
                                    .pl(rems(0.5 + entry.depth() as f32 * 1.0))
                                    .child(
                                        h_flex()
                                            .gap_1p5()
                                            .text_sm()
                                            .child(Icon::new(icon).xsmall().text_color(color))
                                            .child(div().truncate().child(item.label.clone())),
                                    )
                            },
                        ))),
                )
                .child(
                    resizable_panel().child(
                        Editor::new(&self.script)
                            .bordered(false)
                            .h_full()
                            .font_family(theme.mono_font_family.clone())
                            .text_size(theme.mono_font_size),
                    ),
                )
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .p_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_sm().child(t!("compare.source").to_string()))
                    .child(div().w(rems(16.)).child(Select::new(&self.source).small()))
                    .child(Icon::new(IconName::ArrowRight).xsmall())
                    .child(div().text_sm().child(t!("compare.target").to_string()))
                    .child(div().w(rems(16.)).child(Select::new(&self.target).small()))
                    .child(div().flex_1())
                    .child(
                        Button::new("copy-script")
                            .ghost()
                            .small()
                            .icon(gpui_kit::assets::IconName::Copy)
                            .tooltip(t!("compare.copy").to_string())
                            .disabled(!has_script)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let script = this.script.read(cx).value().to_string();
                                cx.write_to_clipboard(ClipboardItem::new_string(script));
                            })),
                    )
                    .child(
                        Button::new("script-to-console")
                            .small()
                            .outline()
                            .icon(IconName::SquareTerminal)
                            .label(t!("compare.to_console").to_string())
                            .disabled(!has_script)
                            .on_click(cx.listener(|this, _, _, cx| this.open_in_console(cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
