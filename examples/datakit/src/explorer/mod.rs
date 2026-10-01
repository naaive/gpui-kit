//! The database explorer: data sources and everything in them — schemas,
//! tables, views, columns, keys, indexes, triggers, routines, sequences —
//! read lazily as they are expanded, and filtered by name.

mod tree_model;

use std::{collections::HashSet, rc::Rc, sync::Arc};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariant},
    dock::{BasePanel, Panel, PanelControl, PanelEvent, PanelInfo, PanelState},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    menu::{PopupMenu, PopupMenuItem},
    notification::Notification,
    scroll::ScrollableElement as _,
    text::TextView,
    tree::{TreeEvent, TreeState, tree},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, ScrollStrategy,
    SharedString, Styled as _, Subscription, WeakEntity, Window, actions, div,
    prelude::FluentBuilder as _, px, rems,
};
use rust_i18n::t;
use serde::{Deserialize, Serialize};

use crate::{
    datasource::{
        CatalogRequest, ConnectionStatus, DataSource, DataSourceEvent, DataSourceForm, DataSources,
        DataSourcesEvent, describe_error,
    },
    objects::{ObjectPath, ObjectRef},
};

use tree_model::{Group, Node, Tone, TreeModel, TreeOptions};

actions!(
    explorer,
    [
        /// Open the selected object: a table's data, a routine's source.
        OpenObject,
        /// Show what the selected object is.
        QuickDocumentation,
        /// Rename the selected table, view or column.
        RenameObject
    ]
);

const CONTEXT: &str = "Explorer";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("f4", OpenObject, Some(CONTEXT)),
        KeyBinding::new("shift-f6", RenameObject, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("f1", QuickDocumentation, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-q", QuickDocumentation, Some(CONTEXT)),
    ]);
}

#[derive(Clone)]
pub enum ExplorerEvent {
    /// Open a console for `data_source` holding `sql`, running it when
    /// `run` is set.
    OpenConsole {
        data_source: Entity<DataSource>,
        sql: Option<String>,
        run: bool,
    },
    /// Ask for a new data source.
    NewDataSource,
    /// Open an object: a relation's data, any other object's source.
    Open(ObjectRef),
    /// Create a table in `schema`, or change `relation`.
    EditTable {
        data_source: Entity<DataSource>,
        schema: Arc<str>,
        relation: Option<Arc<str>>,
    },
    /// Show the relations of a schema and their references.
    ShowDiagram {
        data_source: Entity<DataSource>,
        schema: Arc<str>,
    },
    /// Compare a schema with another.
    CompareSchema {
        data_source: Entity<DataSource>,
        schema: Arc<str>,
    },
    /// Import rows from a file into a table.
    ImportData(ObjectRef),
    /// Compare the rows of a table with another's.
    CompareData(ObjectRef),
    /// Back up or restore a database with the database's own tools.
    DumpOrRestore(Entity<DataSource>),
}

/// The database explorer tool window.
pub struct ExplorerPanel {
    focus_handle: FocusHandle,
    tree: Entity<TreeState>,
    filter: Entity<InputState>,
    nodes: Rc<std::collections::HashMap<SharedString, Node>>,
    expanded: HashSet<SharedString>,
    show_system_schemas: bool,
    _subscriptions: Vec<Subscription>,
    data_source_subscriptions: Vec<Subscription>,
}

#[derive(Default, Serialize, Deserialize)]
struct SavedExplorer {
    #[serde(default)]
    show_system_schemas: bool,
}

impl EventEmitter<PanelEvent> for ExplorerPanel {}
impl EventEmitter<ExplorerEvent> for ExplorerPanel {}

impl ExplorerPanel {
    pub const NAME: &str = "Explorer";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("explorer.filter").to_string()));
        let data_sources = DataSources::global(cx);
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            // Focusing the panel puts the keyboard in the tree.
            cx.on_focus(&focus_handle, window, |this, window, cx| {
                this.tree.update(cx, |tree, cx| tree.focus(window, cx));
            }),
            cx.subscribe(&tree, Self::on_tree_event),
            cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.rebuild(cx);
                }
            }),
            cx.subscribe(&data_sources, |this, _, _: &DataSourcesEvent, cx| {
                this.watch_data_sources(cx);
                this.rebuild(cx);
            }),
        ];
        let mut panel = Self {
            focus_handle,
            tree,
            filter,
            nodes: Rc::new(Default::default()),
            expanded: HashSet::new(),
            show_system_schemas: false,
            _subscriptions: subscriptions,
            data_source_subscriptions: Vec::new(),
        };
        panel.watch_data_sources(cx);
        panel.rebuild(cx);
        panel
    }

    /// Take the view options a saved layout recorded.
    pub fn restore(&mut self, state: &PanelState, cx: &mut Context<Self>) {
        if let PanelInfo::Panel(value) = &state.info
            && let Ok(saved) = serde_json::from_value::<SavedExplorer>(value.clone())
        {
            self.show_system_schemas = saved.show_system_schemas;
            self.rebuild(cx);
        }
    }

    fn selected_node(&self, cx: &App) -> Option<Node> {
        self.tree
            .read(cx)
            .selected_item()
            .and_then(|item| self.nodes.get(&item.id))
            .cloned()
    }

    /// The data source of the selected row, or the only one there is.
    pub fn selected_data_source(&self, cx: &App) -> Option<Entity<DataSource>> {
        self.selected_node(cx)
            .and_then(|node| node.data_source().cloned())
            .or_else(|| {
                let data_sources = DataSources::global(cx);
                let items = data_sources.read(cx).items();
                (items.len() == 1).then(|| items[0].clone())
            })
    }

    /// Select `object` in the tree, expanding and loading what leads to it.
    pub fn reveal(&mut self, object: &ObjectRef, window: &mut Window, cx: &mut Context<Self>) {
        let data_source = object.data_source().clone();
        let source = data_source.read(cx);
        let path = object.path();
        let mut ids = vec![SharedString::from(format!("ds/{}", source.profile().id()))];
        ids.push(tree_model::object_id(
            source,
            &ObjectPath::Schema {
                schema: path.schema().clone(),
            },
        ));
        if let Some(relation) = path.relation_name() {
            let relation_path = ObjectPath::relation(path.schema().clone(), relation.clone());
            let is_view = relation_path
                .resolve(source.catalog())
                .is_some_and(|object| {
                    matches!(object, crate::objects::CatalogObject::Relation(r) if r.relation_type().is_view())
                });
            ids.push(format!("{}/g/{}", ids[1], if is_view { "views" } else { "tables" }).into());
            ids.push(tree_model::object_id(source, &relation_path));
        }
        let target = tree_model::object_id(source, path);
        self.expanded.extend(ids);
        data_source.update(cx, |data_source, cx| {
            data_source.ensure(CatalogRequest::Objects(path.schema().clone()), cx)
        });
        self.rebuild(cx);
        self.tree.update(cx, |tree, cx| {
            tree.reveal_item(&target, ScrollStrategy::Center, cx);
            if let Some(ix) = tree.index_of(&target) {
                tree.set_selected_index(Some(ix), cx);
            }
            tree.focus(window, cx);
        });
    }

    fn watch_data_sources(&mut self, cx: &mut Context<Self>) {
        let items: Vec<Entity<DataSource>> = DataSources::global(cx).read(cx).items().to_vec();
        self.data_source_subscriptions = items
            .iter()
            .map(|item| cx.subscribe(item, |this, _, _: &DataSourceEvent, cx| this.rebuild(cx)))
            .collect();
    }

    fn on_tree_event(&mut self, _: Entity<TreeState>, event: &TreeEvent, cx: &mut Context<Self>) {
        match event {
            TreeEvent::Expanded(id) => {
                self.expanded.insert(id.clone());
                match self.nodes.get(id).cloned() {
                    Some(Node::DataSource(data_source)) => {
                        data_source.update(cx, |data_source, cx| {
                            data_source.ensure(CatalogRequest::Schemas, cx);
                        })
                    }
                    Some(Node::Object { object, .. })
                        if matches!(object.path(), ObjectPath::Schema { .. }) =>
                    {
                        let schema = object.path().schema().clone();
                        object.data_source().update(cx, |data_source, cx| {
                            data_source.ensure(CatalogRequest::Objects(schema), cx)
                        })
                    }
                    _ => {}
                }
            }
            TreeEvent::Collapsed(id) => {
                self.expanded.remove(id);
            }
        }
    }

    /// Build the tree again from the data sources and their catalogs,
    /// keeping what is expanded and selected.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter.read(cx).value().trim().to_lowercase();
        let items = DataSources::global(cx).read(cx).items().to_vec();
        let TreeModel { items, nodes } = tree_model::build(
            &items,
            &TreeOptions {
                expanded: &self.expanded,
                filter: &filter,
                show_system_schemas: self.show_system_schemas,
            },
            cx,
        );
        self.nodes = Rc::new(nodes);
        let selected = self
            .tree
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone());
        self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            if let Some(ix) = selected.and_then(|id| tree.index_of(&id)) {
                tree.set_selected_index(Some(ix), cx);
            }
        });
        cx.notify();
    }

    fn open_selected(&mut self, _: &OpenObject, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(node) = self.selected_node(cx) {
            self.open(&node, window, cx);
        }
    }

    fn open(&mut self, node: &Node, window: &mut Window, cx: &mut Context<Self>) {
        if let Node::Role { role, .. } = node {
            show_role(role, window, cx);
            return;
        }
        if let Node::Object { object, .. } = node
            && !matches!(
                object.path(),
                ObjectPath::Schema { .. } | ObjectPath::Column { .. }
            )
        {
            cx.emit(ExplorerEvent::Open(object.clone()));
        }
    }

    fn quick_documentation(
        &mut self,
        _: &QuickDocumentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.selected_node(cx) {
            Some(Node::Role { role, .. }) => show_role(&role, window, cx),
            Some(node) => {
                if let Some(object) = node.object() {
                    show_documentation(object, window, cx);
                }
            }
            None => {}
        }
    }

    fn rename_selected(&mut self, _: &RenameObject, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(object) = self
            .selected_node(cx)
            .and_then(|node| node.object().cloned())
        {
            crate::rename::rename_object(&object, window, cx);
        }
    }

    fn context_menu(
        explorer: WeakEntity<Self>,
        node: Option<Node>,
        menu: PopupMenu,
        cx: &App,
    ) -> PopupMenu {
        let item =
            |label: SharedString,
             action: Rc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>| {
                let explorer = explorer.clone();
                PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    let action = action.clone();
                    let _ = explorer.update(cx, |explorer, cx| action(explorer, window, cx));
                })
            };
        let emit = |label: SharedString, event: ExplorerEvent| {
            item(
                label,
                Rc::new(move |_, _, cx: &mut Context<Self>| cx.emit(event.clone())),
            )
        };
        match node {
            Some(Node::DataSource(data_source)) => {
                let connected = matches!(
                    data_source.read(cx).status(),
                    ConnectionStatus::Connected | ConnectionStatus::Connecting
                );
                let toggle = data_source.clone();
                let refresh = data_source.clone();
                let edit = data_source.clone();
                let remove = data_source.clone();
                menu.item(emit(
                    t!("explorer.new_console").into(),
                    ExplorerEvent::OpenConsole {
                        data_source: data_source.clone(),
                        sql: None,
                        run: false,
                    },
                ))
                .separator()
                .item(item(
                    if connected {
                        t!("explorer.disconnect").into()
                    } else {
                        t!("explorer.connect").into()
                    },
                    Rc::new(move |_, _, cx| {
                        toggle.update(cx, |data_source, cx| {
                            if connected {
                                data_source.disconnect(cx)
                            } else {
                                data_source.request(CatalogRequest::Schemas, cx)
                            }
                        })
                    }),
                ))
                .item(item(
                    t!("explorer.refresh").into(),
                    Rc::new(move |_, _, cx| {
                        refresh.update(cx, |data_source, cx| data_source.refresh(cx))
                    }),
                ))
                .separator()
                .item(emit(
                    t!("explorer.dump").into(),
                    ExplorerEvent::DumpOrRestore(data_source.clone()),
                ))
                .separator()
                .item(item(
                    t!("explorer.properties").into(),
                    Rc::new(move |_, window, cx| DataSourceForm::open(Some(&edit), window, cx)),
                ))
                .item(item(
                    t!("explorer.remove").into(),
                    Rc::new(move |_, window, cx| confirm_remove(&remove, window, cx)),
                ))
            }
            Some(Node::Group {
                data_source,
                schema,
                group,
            }) => {
                let menu = match group {
                    Group::Tables => menu.item(emit(
                        t!("explorer.new_table").into(),
                        ExplorerEvent::EditTable {
                            data_source: data_source.clone(),
                            schema: schema.clone(),
                            relation: None,
                        },
                    )),
                    _ => menu,
                };
                menu.item(emit(
                    t!("explorer.new_console").into(),
                    ExplorerEvent::OpenConsole {
                        data_source,
                        sql: None,
                        run: false,
                    },
                ))
            }
            Some(Node::Object { object, .. }) => Self::object_menu(&object, menu, &item, &emit, cx),
            Some(Node::Role { data_source, role }) => {
                let name = role.name().to_string();
                let documented = role.clone();
                let menu = match role.definition() {
                    Some(definition) => {
                        let ddl = definition.to_string();
                        menu.item(emit(
                            t!("explorer.ddl_to_console").into(),
                            ExplorerEvent::OpenConsole {
                                data_source,
                                sql: Some(ddl.clone()),
                                run: false,
                            },
                        ))
                        .item(item(
                            t!("explorer.copy_ddl").into(),
                            Rc::new(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(ddl.clone()))
                            }),
                        ))
                        .separator()
                    }
                    None => menu,
                };
                menu.item(item(
                    t!("explorer.copy_name").into(),
                    Rc::new(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(name.clone()))
                    }),
                ))
                .item(item(
                    t!("explorer.quick_documentation").into(),
                    Rc::new(move |_, window, cx| show_role(&documented, window, cx)),
                ))
            }
            _ => menu,
        }
    }

    fn object_menu(
        object: &ObjectRef,
        menu: PopupMenu,
        item: &dyn Fn(
            SharedString,
            Rc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>,
        ) -> PopupMenuItem,
        emit: &dyn Fn(SharedString, ExplorerEvent) -> PopupMenuItem,
        cx: &App,
    ) -> PopupMenu {
        let data_source = object.data_source().clone();
        let path = object.path().clone();
        let schema = path.schema().clone();
        let is_table = match path.resolve(data_source.read(cx).catalog()) {
            Some(crate::objects::CatalogObject::Relation(relation)) => {
                relation.relation_type().is_table()
            }
            _ => false,
        };
        let name = object.qualified_name(cx);
        let ddl = object.ddl(cx);
        let mut menu = menu;
        match &path {
            ObjectPath::Schema { .. } => {
                menu = menu
                    .item(emit(
                        t!("explorer.new_console").into(),
                        ExplorerEvent::OpenConsole {
                            data_source: data_source.clone(),
                            sql: None,
                            run: false,
                        },
                    ))
                    .item(emit(
                        t!("explorer.new_table").into(),
                        ExplorerEvent::EditTable {
                            data_source: data_source.clone(),
                            schema: schema.clone(),
                            relation: None,
                        },
                    ))
                    .separator()
                    .item(emit(
                        t!("explorer.diagram").into(),
                        ExplorerEvent::ShowDiagram {
                            data_source: data_source.clone(),
                            schema: schema.clone(),
                        },
                    ))
                    .item(emit(
                        t!("explorer.compare").into(),
                        ExplorerEvent::CompareSchema {
                            data_source: data_source.clone(),
                            schema: schema.clone(),
                        },
                    ))
                    .item({
                        let data_source = data_source.clone();
                        let schema = schema.clone();
                        item(
                            t!("explorer.refresh").into(),
                            Rc::new(move |_, _, cx| {
                                data_source.update(cx, |data_source, cx| {
                                    data_source.request(CatalogRequest::Objects(schema.clone()), cx)
                                })
                            }),
                        )
                    });
            }
            ObjectPath::Relation { .. } => {
                let select = object.select(Some(100), cx);
                menu = menu
                    .item(emit(
                        t!("explorer.open_data").into(),
                        ExplorerEvent::Open(object.clone()),
                    ))
                    .item(emit(
                        t!("explorer.new_console_select").into(),
                        ExplorerEvent::OpenConsole {
                            data_source: data_source.clone(),
                            sql: select,
                            run: false,
                        },
                    ));
                if is_table {
                    menu = menu
                        .separator()
                        .item(emit(
                            t!("explorer.modify_table").into(),
                            ExplorerEvent::EditTable {
                                data_source: data_source.clone(),
                                schema: schema.clone(),
                                relation: Some(path.name()),
                            },
                        ))
                        .item(emit(
                            t!("explorer.import").into(),
                            ExplorerEvent::ImportData(object.clone()),
                        ))
                        .item(emit(
                            t!("explorer.compare_data").into(),
                            ExplorerEvent::CompareData(object.clone()),
                        ));
                }
            }
            ObjectPath::Routine { .. }
            | ObjectPath::Sequence { .. }
            | ObjectPath::Trigger { .. } => {
                menu = menu.item(emit(
                    t!("explorer.open_source").into(),
                    ExplorerEvent::Open(object.clone()),
                ));
            }
            _ => {}
        }
        if let Some(ddl) = ddl {
            menu = menu
                .separator()
                .item(emit(
                    t!("explorer.ddl_to_console").into(),
                    ExplorerEvent::OpenConsole {
                        data_source: data_source.clone(),
                        sql: Some(ddl.clone()),
                        run: false,
                    },
                ))
                .item(item(
                    t!("explorer.copy_ddl").into(),
                    Rc::new(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(ddl.clone()))
                    }),
                ));
        }
        let documented = object.clone();
        let dropped = object.clone();
        let renamed = object.clone();
        if object.target().is_some() {
            menu = menu.separator().item(item(
                t!("explorer.rename").into(),
                Rc::new(move |_, window, cx| crate::rename::rename_object(&renamed, window, cx)),
            ));
        }
        menu = menu
            .item(item(
                t!("explorer.copy_name").into(),
                Rc::new(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(name.clone()))
                }),
            ))
            .item(item(
                t!("explorer.quick_documentation").into(),
                Rc::new(move |_, window, cx| show_documentation(&documented, window, cx)),
            ));
        if dropped.drop_statement(cx).is_some() {
            menu = menu.separator().item(item(
                t!("explorer.drop").into(),
                Rc::new(move |_, window, cx| confirm_drop(&dropped, window, cx)),
            ));
        }
        menu
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let load_error = DataSources::global(cx).read(cx).load_error().cloned();
        v_flex()
            .size_full()
            .p_4()
            .gap_3()
            .items_center()
            .justify_center()
            .text_sm()
            .text_center()
            .child(
                Icon::new(IconName::Database)
                    .size_8()
                    .text_color(cx.theme().muted_foreground),
            )
            .map(|this| match load_error {
                Some(error) => this.child(
                    div()
                        .text_color(cx.theme().danger)
                        .child(t!("explorer.load_failed", error = error).to_string()),
                ),
                None => this
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("explorer.empty").to_string()),
                    )
                    .child(
                        Button::new("new-data-source")
                            .icon(IconName::Plus)
                            .label(t!("explorer.new_data_source").to_string())
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ExplorerEvent::NewDataSource)),
                            ),
                    ),
            })
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let nodes = self.nodes.clone();
        let explorer = cx.entity().downgrade();
        let menu_nodes = self.nodes.clone();
        let menu_explorer = explorer.clone();
        tree(&self.tree, move |ix, entry, selected, _, cx| {
            let item = entry.item();
            let node = nodes.get(&item.id).cloned();
            let theme = cx.theme();
            let muted = theme.muted_foreground;
            let (icon, color, detail): (Option<IconName>, _, Option<SharedString>) = match &node {
                Some(Node::DataSource(data_source)) => {
                    let source = data_source.read(cx);
                    let color = match source.status() {
                        ConnectionStatus::Connected => theme.success,
                        ConnectionStatus::Failed(_) => theme.danger,
                        _ => muted,
                    };
                    (
                        Some(IconName::Database),
                        color,
                        Some(source.profile().address().into()),
                    )
                }
                Some(Node::Role { role, .. }) => (
                    Some(if role.can_login() {
                        IconName::User
                    } else {
                        IconName::Users
                    }),
                    muted,
                    (!role.attributes().is_empty()).then(|| role.attributes().join(" ").into()),
                ),
                Some(Node::Group { .. }) => (
                    Some(if entry.is_expanded() {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    }),
                    muted,
                    None,
                ),
                Some(Node::Object {
                    icon, tone, detail, ..
                }) => (
                    Some(*icon),
                    match tone {
                        Tone::Accent => theme.primary,
                        Tone::Muted => muted,
                    },
                    detail.clone(),
                ),
                Some(Node::Message { failed }) => (
                    failed.then_some(IconName::TriangleAlert),
                    if *failed { theme.danger } else { muted },
                    None,
                ),
                None => (None, muted, None),
            };
            // A data source's mark: its color and whether it is read-only.
            let (mark, locked) = match &node {
                Some(Node::DataSource(data_source)) => {
                    let source = data_source.read(cx);
                    (
                        source.color().map(|color| color.hsla(cx)),
                        source.is_read_only(),
                    )
                }
                _ => (None, false),
            };
            let is_message = matches!(node, Some(Node::Message { .. }));
            let failed = matches!(node, Some(Node::Message { failed: true }));
            let danger = theme.danger;
            let explorer = explorer.clone();
            // A fixed slot keeps labels on one line whether or not the row
            // can expand.
            let disclosure = div().flex_none().size_3().when(entry.is_folder(), |slot| {
                slot.child(
                    Icon::new(if entry.is_expanded() {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .xsmall()
                    .text_color(muted),
                )
            });
            ListItem::new(ix)
                .selected(selected)
                .py_0p5()
                .pl(rems(0.25 + entry.depth() as f32 * 1.0))
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_1p5()
                        .text_sm()
                        .child(disclosure)
                        .when_some(icon, |row, icon| {
                            row.child(Icon::new(icon).xsmall().text_color(color))
                        })
                        .child(
                            div()
                                .flex_shrink_0()
                                .truncate()
                                .when(is_message, |label| {
                                    label.text_color(if failed { danger } else { muted })
                                })
                                .child(item.label.clone()),
                        )
                        .when_some(mark, |row, mark| {
                            row.child(div().flex_none().size_2().rounded_full().bg(mark))
                        })
                        .when(locked, |row| {
                            row.child(Icon::new(IconName::Lock).xsmall().text_color(muted))
                        })
                        .when_some(detail, |row, detail| {
                            row.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(detail),
                            )
                        }),
                )
                .on_click(move |event, window, cx| {
                    if event.click_count() == 2
                        && let Some(node) = &node
                    {
                        let _ = explorer.update(cx, |explorer, cx| explorer.open(node, window, cx));
                    }
                })
        })
        .context_menu(move |_, entry, menu, _, cx| {
            let node = menu_nodes.get(&entry.item().id).cloned();
            Self::context_menu(menu_explorer.clone(), node, menu, cx)
        })
    }
}

/// Show what `object` is in a dialog.
pub fn show_documentation(object: &ObjectRef, window: &mut Window, cx: &mut App) {
    if let Some(text) = object.documentation(cx) {
        show_markdown(text.into(), window, cx);
    }
}

/// Show what a user or role is and may do.
fn show_role(role: &datakit_catalog::Role, window: &mut Window, cx: &mut App) {
    let mut text = format!(
        "**{}** · {}",
        role.name(),
        if role.can_login() {
            t!("explorer.user")
        } else {
            t!("explorer.role")
        }
    );
    if !role.attributes().is_empty() {
        text.push_str(&format!("\n\n`{}`", role.attributes().join(" ")));
    }
    if !role.member_of().is_empty() {
        text.push_str(&format!(
            "\n\n{}",
            t!("explorer.member_of", roles = role.member_of().join(", "))
        ));
    }
    if let Some(definition) = role.definition() {
        text.push_str(&format!("\n\n```sql\n{definition}\n```"));
    }
    show_markdown(text.into(), window, cx);
}

fn show_markdown(text: SharedString, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, _| {
        let text = text.clone();
        dialog
            .title(t!("explorer.quick_documentation").to_string())
            .w(px(520.))
            .content(move |content, _, _| {
                content.child(
                    div()
                        .id("documentation")
                        .max_h(px(420.))
                        .overflow_y_scrollbar()
                        .child(TextView::markdown("documentation", text.clone()).selectable(true)),
                )
            })
    });
}

fn confirm_remove(data_source: &Entity<DataSource>, window: &mut Window, cx: &mut App) {
    let name = data_source.read(cx).name();
    let id = data_source.read(cx).profile().id().clone();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let id = id.clone();
        alert
            .title(t!("explorer.remove_title", name = name).to_string())
            .description(t!("explorer.remove_description").to_string())
            .show_cancel(true)
            .cancel_text(t!("common.cancel").to_string())
            .ok_text(t!("explorer.remove_confirm").to_string())
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                DataSources::global(cx).update(cx, |data_sources, cx| data_sources.remove(&id, cx));
                true
            })
    });
}

/// Ask before dropping `object`, then drop it and read its schema again.
fn confirm_drop(object: &ObjectRef, window: &mut Window, cx: &mut App) {
    let Some(statement) = object.drop_statement(cx) else {
        return;
    };
    let name = object.path().name();
    let object = object.clone();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let object = object.clone();
        let statement = statement.clone();
        alert
            .title(t!("explorer.drop_title", name = name).to_string())
            .description(statement.clone())
            .show_cancel(true)
            .cancel_text(t!("common.cancel").to_string())
            .ok_text(t!("explorer.drop_confirm").to_string())
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                let object = object.clone();
                let data_source = object.data_source().clone();
                let schema = object.path().schema().clone();
                let task = data_source
                    .read(cx)
                    .run_statements(vec![statement.clone()], cx);
                window
                    .spawn(cx, async move |cx| {
                        let result = task.await;
                        let _ = cx.update(|window, cx| match result {
                            Ok(()) => data_source.update(cx, |data_source, cx| {
                                let request = if matches!(object.path(), ObjectPath::Schema { .. })
                                {
                                    CatalogRequest::Schemas
                                } else {
                                    CatalogRequest::Objects(schema)
                                };
                                data_source.request(request, cx)
                            }),
                            Err(error) => window.push_notification(
                                Notification::error(describe_error(&error).to_string()),
                                cx,
                            ),
                        });
                    })
                    .detach();
                true
            })
    });
}

impl Focusable for ExplorerPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for ExplorerPanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }

    fn closable(&self, _: &App) -> bool {
        false
    }

    fn dump(&self, _: &App) -> PanelState {
        let mut state = PanelState::new(Self::NAME);
        let saved = SavedExplorer {
            show_system_schemas: self.show_system_schemas,
        };
        state.info = PanelInfo::panel(serde_json::to_value(saved).unwrap_or_default());
        state
    }
}

impl Panel for ExplorerPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("explorer.title").to_string()
    }

    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let explorer = cx.entity().downgrade();
        let refresh = cx.entity().downgrade();
        let options = cx.entity().downgrade();
        let show_system = self.show_system_schemas;
        Some(vec![
            Button::new("explorer-new-data-source")
                .icon(IconName::Plus)
                .tooltip(t!("explorer.new_data_source").to_string())
                .on_click(move |_, _, cx| {
                    let _ = explorer.update(cx, |_, cx| cx.emit(ExplorerEvent::NewDataSource));
                }),
            Button::new("explorer-refresh")
                .icon(IconName::RefreshCw)
                .tooltip(t!("explorer.refresh").to_string())
                .on_click(move |_, _, cx| {
                    let _ = refresh.update(cx, |explorer, cx| {
                        if let Some(data_source) = explorer.selected_data_source(cx) {
                            data_source.update(cx, |data_source, cx| data_source.refresh(cx));
                        }
                    });
                }),
            Button::new("explorer-show-system")
                .icon(IconName::Eye)
                .selected(show_system)
                .tooltip(t!("explorer.show_system_schemas").to_string())
                .on_click(move |_, _, cx| {
                    let _ = options.update(cx, |explorer, cx| {
                        explorer.show_system_schemas = !explorer.show_system_schemas;
                        explorer.rebuild(cx);
                        cx.emit(PanelEvent::LayoutChanged);
                    });
                }),
        ])
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        None
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for ExplorerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if DataSources::global(cx).read(cx).items().is_empty() {
            return self.render_empty(cx).into_any_element();
        }
        v_flex()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_selected))
            .on_action(cx.listener(Self::quick_documentation))
            .on_action(cx.listener(Self::rename_selected))
            .child(
                div().flex_none().p_1().child(
                    Input::new(&self.filter)
                        .small()
                        .prefix(Icon::new(IconName::Search).xsmall())
                        .cleanable(true),
                ),
            )
            .child(div().flex_1().min_h_0().p_1().child(self.render_tree(cx)))
            .into_any_element()
    }
}
