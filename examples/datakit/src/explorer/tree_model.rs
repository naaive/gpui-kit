//! The explorer's rows, built from the data sources and their catalogs.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use datakit_catalog::{ConstraintRule, Relation, RelationType, Role, Schema};
use gpui_kit::{App, Entity, SharedString, assets::IconName, component::tree::TreeItem};
use rust_i18n::t;

use crate::{
    datasource::{CatalogRequest, ConnectionStatus, DataSource},
    objects::{self, ObjectPath, ObjectRef},
};

/// What a row stands for.
#[derive(Clone)]
pub(crate) enum Node {
    DataSource(Entity<DataSource>),
    Object {
        object: ObjectRef,
        icon: IconName,
        tone: Tone,
        detail: Option<SharedString>,
    },
    Group {
        data_source: Entity<DataSource>,
        schema: Arc<str>,
        group: Group,
    },
    /// A user or role of the server.
    Role {
        data_source: Entity<DataSource>,
        role: Role,
    },
    Message {
        failed: bool,
    },
}

impl Node {
    pub(crate) fn data_source(&self) -> Option<&Entity<DataSource>> {
        match self {
            Node::DataSource(data_source)
            | Node::Group { data_source, .. }
            | Node::Role { data_source, .. } => Some(data_source),
            Node::Object { object, .. } => Some(object.data_source()),
            Node::Message { .. } => None,
        }
    }

    pub(crate) fn object(&self) -> Option<&ObjectRef> {
        match self {
            Node::Object { object, .. } => Some(object),
            _ => None,
        }
    }
}

/// How strongly a row's icon is colored.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Objects people open: tables, views, routines.
    Accent,
    Muted,
}

/// A folder of objects of one sort.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Group {
    Tables,
    Views,
    Routines,
    Sequences,
    Keys,
    ForeignKeys,
    Indexes,
    Checks,
    Triggers,
    /// The server's users and roles, under the data source.
    Roles,
    /// The keys of a key-value store.
    Values,
}

impl Group {
    fn key(self) -> &'static str {
        match self {
            Group::Tables => "tables",
            Group::Views => "views",
            Group::Routines => "routines",
            Group::Sequences => "sequences",
            Group::Keys => "keys",
            Group::ForeignKeys => "foreign-keys",
            Group::Indexes => "indexes",
            Group::Checks => "checks",
            Group::Triggers => "triggers",
            Group::Roles => "roles",
            Group::Values => "values",
        }
    }

    fn label(self, count: usize) -> SharedString {
        match self {
            Group::Tables => t!("explorer.tables", count = count),
            Group::Views => t!("explorer.views", count = count),
            Group::Routines => t!("explorer.routines", count = count),
            Group::Sequences => t!("explorer.sequences", count = count),
            Group::Keys => t!("explorer.keys", count = count),
            Group::ForeignKeys => t!("explorer.foreign_keys", count = count),
            Group::Indexes => t!("explorer.indexes", count = count),
            Group::Checks => t!("explorer.checks", count = count),
            Group::Triggers => t!("explorer.triggers", count = count),
            Group::Roles => t!("explorer.roles", count = count),
            Group::Values => t!("explorer.values", count = count),
        }
        .into()
    }
}

/// What the tree shows besides the data sources themselves.
pub(crate) struct TreeOptions<'a> {
    pub expanded: &'a HashSet<SharedString>,
    /// Lower-case text a relation, routine or sequence name must contain;
    /// empty shows everything.
    pub filter: &'a str,
    pub show_system_schemas: bool,
}

pub(crate) struct TreeModel {
    pub items: Vec<TreeItem>,
    pub nodes: HashMap<SharedString, Node>,
}

pub(crate) fn build(
    data_sources: &[Entity<DataSource>],
    options: &TreeOptions,
    cx: &App,
) -> TreeModel {
    let mut builder = Builder {
        options,
        nodes: HashMap::new(),
    };
    let items = data_sources
        .iter()
        .map(|data_source| builder.data_source(data_source, cx))
        .collect();
    TreeModel {
        items,
        nodes: builder.nodes,
    }
}

/// The tree id of `path` in `data_source`, for revealing it.
pub(crate) fn object_id(data_source: &DataSource, path: &ObjectPath) -> SharedString {
    format!("{}/{}", data_source_id(data_source), path.key()).into()
}

fn data_source_id(data_source: &DataSource) -> String {
    format!("ds/{}", data_source.profile().id())
}

struct Builder<'a> {
    options: &'a TreeOptions<'a>,
    nodes: HashMap<SharedString, Node>,
}

impl Builder<'_> {
    fn filtering(&self) -> bool {
        !self.options.filter.is_empty()
    }

    fn matches(&self, name: &str) -> bool {
        name.to_lowercase().contains(self.options.filter)
    }

    /// A folder row, open when the person opened it or a filter is showing
    /// what is inside.
    fn folder(&self, id: String, label: impl Into<SharedString>) -> TreeItem {
        let expanded = self.filtering() || self.options.expanded.contains(id.as_str());
        TreeItem::new(id, label).expanded(expanded)
    }

    fn leaf(&mut self, id: String, label: impl Into<SharedString>, node: Node) -> TreeItem {
        self.nodes.insert(id.clone().into(), node);
        TreeItem::new(id, label)
    }

    fn data_source(&mut self, data_source: &Entity<DataSource>, cx: &App) -> TreeItem {
        let source = data_source.read(cx);
        let id = data_source_id(source);
        self.nodes
            .insert(id.clone().into(), Node::DataSource(data_source.clone()));
        let catalog = source.catalog();
        let children: Vec<TreeItem> = if catalog.has_schemas() {
            let mut children: Vec<TreeItem> = catalog
                .schemas()
                .iter()
                .filter(|schema| self.options.show_system_schemas || !schema.is_system())
                .filter_map(|schema| self.schema(&id, data_source, schema, cx))
                .collect();
            children.extend(self.roles(&id, data_source, catalog.roles()));
            children
        } else {
            let failure =
                source
                    .failure(&CatalogRequest::Schemas)
                    .cloned()
                    .or(match source.status() {
                        ConnectionStatus::Failed(error) => Some(error.clone()),
                        _ => None,
                    });
            vec![self.message(&id, failure)]
        };
        // A data source stays closed under a filter unless the person
        // opened it: opening it is what connects.
        let expanded = self.options.expanded.contains(id.as_str());
        TreeItem::new(id, source.name())
            .expanded(expanded)
            .children(children)
    }

    fn schema(
        &mut self,
        parent: &str,
        data_source: &Entity<DataSource>,
        schema: &Schema,
        cx: &App,
    ) -> Option<TreeItem> {
        let path = ObjectPath::Schema {
            schema: schema.name(),
        };
        let id = format!("{parent}/{}", path.key());
        let children = match schema.relations() {
            Some(relations) => {
                let mut groups = Vec::new();
                let (keys, relations): (Vec<&Relation>, Vec<&Relation>) = relations
                    .iter()
                    .filter(|relation| self.matches(&relation.name()))
                    .partition(|relation| relation.relation_type() == RelationType::Key);
                let (views, tables): (Vec<&Relation>, Vec<&Relation>) = relations
                    .into_iter()
                    .partition(|relation| relation.relation_type().is_view());
                for (group, relations) in [
                    (Group::Tables, tables),
                    (Group::Views, views),
                    (Group::Values, keys),
                ] {
                    if relations.is_empty() {
                        continue;
                    }
                    let group_id = format!("{id}/g/{}", group.key());
                    let children: Vec<TreeItem> = relations
                        .iter()
                        .map(|relation| self.relation(parent, data_source, schema, relation))
                        .collect();
                    groups.push(self.group(group_id, data_source, schema, group, children));
                }
                let filter = self.options.filter;
                let matches = |name: &str| name.to_lowercase().contains(filter);
                let routines: Vec<TreeItem> = schema
                    .routines()
                    .iter()
                    .filter(|routine| matches(&routine.name()))
                    .map(|routine| {
                        let path = ObjectPath::Routine {
                            schema: schema.name(),
                            signature: routine.signature().into(),
                        };
                        let routine_id = format!("{parent}/{}", path.key());
                        self.leaf(
                            routine_id,
                            format!("{}({})", routine.name(), routine.arguments()),
                            Node::Object {
                                object: ObjectRef::new(data_source.clone(), path),
                                icon: IconName::SquareFunction,
                                tone: Tone::Accent,
                                detail: routine.result().map(|r| r.to_string().into()),
                            },
                        )
                    })
                    .collect();
                if !routines.is_empty() {
                    let group_id = format!("{id}/g/{}", Group::Routines.key());
                    groups.push(self.group(
                        group_id,
                        data_source,
                        schema,
                        Group::Routines,
                        routines,
                    ));
                }
                let sequences: Vec<TreeItem> = schema
                    .sequences()
                    .iter()
                    .filter(|sequence| matches(&sequence.name()))
                    .map(|sequence| {
                        let path = ObjectPath::Sequence {
                            schema: schema.name(),
                            sequence: sequence.name(),
                        };
                        let sequence_id = format!("{parent}/{}", path.key());
                        self.leaf(
                            sequence_id,
                            sequence.name().to_string(),
                            Node::Object {
                                object: ObjectRef::new(data_source.clone(), path),
                                icon: IconName::Hash,
                                tone: Tone::Muted,
                                detail: sequence.owned_by().map(|o| o.to_string().into()),
                            },
                        )
                    })
                    .collect();
                if !sequences.is_empty() {
                    let group_id = format!("{id}/g/{}", Group::Sequences.key());
                    groups.push(self.group(
                        group_id,
                        data_source,
                        schema,
                        Group::Sequences,
                        sequences,
                    ));
                }
                if self.filtering() && groups.is_empty() {
                    return None;
                }
                groups
            }
            None if self.filtering() => return None,
            None => {
                let failure = data_source
                    .read(cx)
                    .failure(&CatalogRequest::Objects(schema.name()))
                    .cloned();
                vec![self.message(&id, failure)]
            }
        };
        self.nodes.insert(
            id.clone().into(),
            Node::Object {
                object: ObjectRef::new(data_source.clone(), path),
                icon: IconName::Braces,
                tone: Tone::Muted,
                detail: None,
            },
        );
        Some(
            self.folder(id, schema.name().to_string())
                .children(children),
        )
    }

    /// The folder of the server's users and roles, when there are any the
    /// filter lets through.
    fn roles(
        &mut self,
        parent: &str,
        data_source: &Entity<DataSource>,
        roles: &[Role],
    ) -> Option<TreeItem> {
        let id = format!("{parent}/g/{}", Group::Roles.key());
        let shown: Vec<&Role> = roles
            .iter()
            .filter(|role| self.matches(&role.name()))
            .collect();
        let children: Vec<TreeItem> = shown
            .into_iter()
            .map(|role| {
                self.leaf(
                    format!("{id}/{}", role.name()),
                    role.name().to_string(),
                    Node::Role {
                        data_source: data_source.clone(),
                        role: role.clone(),
                    },
                )
            })
            .collect();
        if children.is_empty() {
            return None;
        }
        self.nodes.insert(
            id.clone().into(),
            Node::Group {
                data_source: data_source.clone(),
                schema: "".into(),
                group: Group::Roles,
            },
        );
        let label = Group::Roles.label(children.len());
        Some(self.folder(id, label).children(children))
    }

    fn group(
        &mut self,
        id: String,
        data_source: &Entity<DataSource>,
        schema: &Schema,
        group: Group,
        children: Vec<TreeItem>,
    ) -> TreeItem {
        self.nodes.insert(
            id.clone().into(),
            Node::Group {
                data_source: data_source.clone(),
                schema: schema.name(),
                group,
            },
        );
        let label = group.label(children.len());
        self.folder(id, label).children(children)
    }

    fn relation(
        &mut self,
        parent: &str,
        data_source: &Entity<DataSource>,
        schema: &Schema,
        relation: &Relation,
    ) -> TreeItem {
        let path = ObjectPath::relation(schema.name(), relation.name());
        let id = format!("{parent}/{}", path.key());
        self.nodes.insert(
            id.clone().into(),
            Node::Object {
                object: ObjectRef::new(data_source.clone(), path),
                icon: objects::relation_icon(relation.relation_type()),
                tone: Tone::Accent,
                detail: relation.comment().map(|comment| {
                    comment
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string()
                        .into()
                }),
            },
        );
        // A key's value has no columns of its own to list.
        if relation.relation_type() == RelationType::Key {
            return TreeItem::new(id, relation.name().to_string());
        }
        let object = |path: ObjectPath| ObjectRef::new(data_source.clone(), path);
        let mut children: Vec<TreeItem> = relation
            .columns()
            .iter()
            .map(|column| {
                let path = ObjectPath::Column {
                    schema: schema.name(),
                    relation: relation.name(),
                    column: column.name(),
                };
                let column_id = format!("{parent}/{}", path.key());
                let icon = if column.is_primary_key() {
                    IconName::Key
                } else if relation
                    .foreign_keys()
                    .any(|(_, key)| key.columns().contains(&column.name()))
                {
                    IconName::Link
                } else {
                    IconName::Columns3
                };
                let mut detail = column.data_type().to_string();
                if !column.is_nullable() && !column.is_primary_key() {
                    detail.push_str(" · NOT NULL");
                }
                self.leaf(
                    column_id,
                    column.name().to_string(),
                    Node::Object {
                        object: object(path),
                        icon,
                        tone: Tone::Muted,
                        detail: Some(detail.into()),
                    },
                )
            })
            .collect();

        let constraint_group =
            |builder: &mut Self, group: Group, accept: fn(&ConstraintRule) -> bool| {
                let items: Vec<TreeItem> = relation
                    .constraints()
                    .iter()
                    .filter(|constraint| accept(constraint.rule()))
                    .map(|constraint| {
                        let path = ObjectPath::Constraint {
                            schema: schema.name(),
                            relation: relation.name(),
                            constraint: constraint.name(),
                        };
                        let constraint_id = format!("{parent}/{}", path.key());
                        let detail = match constraint.rule() {
                            ConstraintRule::ForeignKey(key) => format!(
                                "({}) → {}",
                                key.columns().join(", "),
                                key.referenced_relation()
                            ),
                            ConstraintRule::Check { expression } => expression.to_string(),
                            rule => format!("({})", rule.columns().join(", ")),
                        };
                        builder.leaf(
                            constraint_id,
                            constraint.name().to_string(),
                            Node::Object {
                                object: ObjectRef::new(data_source.clone(), path),
                                icon: objects::constraint_icon(constraint),
                                tone: Tone::Muted,
                                detail: Some(detail.into()),
                            },
                        )
                    })
                    .collect();
                (!items.is_empty()).then(|| {
                    let group_id = format!("{id}/g/{}", group.key());
                    builder.group(group_id, data_source, schema, group, items)
                })
            };
        if relation.relation_type() != RelationType::View {
            children.extend(constraint_group(self, Group::Keys, |rule| {
                matches!(
                    rule,
                    ConstraintRule::PrimaryKey { .. } | ConstraintRule::Unique { .. }
                )
            }));
            children.extend(constraint_group(self, Group::ForeignKeys, |rule| {
                matches!(rule, ConstraintRule::ForeignKey(_))
            }));
            children.extend(constraint_group(self, Group::Checks, |rule| {
                matches!(
                    rule,
                    ConstraintRule::Check { .. } | ConstraintRule::Exclusion
                )
            }));
            let indexes: Vec<TreeItem> = relation
                .indexes()
                .iter()
                .map(|index| {
                    let path = ObjectPath::Index {
                        schema: schema.name(),
                        relation: relation.name(),
                        index: index.name(),
                    };
                    let index_id = format!("{parent}/{}", path.key());
                    let mut detail = format!("({})", index.columns().join(", "));
                    if index.is_unique() {
                        detail.push_str(" · UNIQUE");
                    }
                    self.leaf(
                        index_id,
                        index.name().to_string(),
                        Node::Object {
                            object: object(path),
                            icon: IconName::ListOrdered,
                            tone: Tone::Muted,
                            detail: Some(detail.into()),
                        },
                    )
                })
                .collect();
            if !indexes.is_empty() {
                let group_id = format!("{id}/g/{}", Group::Indexes.key());
                children.push(self.group(group_id, data_source, schema, Group::Indexes, indexes));
            }
            let triggers: Vec<TreeItem> = relation
                .triggers()
                .iter()
                .map(|trigger| {
                    let path = ObjectPath::Trigger {
                        schema: schema.name(),
                        relation: relation.name(),
                        trigger: trigger.name(),
                    };
                    let trigger_id = format!("{parent}/{}", path.key());
                    self.leaf(
                        trigger_id,
                        trigger.name().to_string(),
                        Node::Object {
                            object: object(path),
                            icon: IconName::Zap,
                            tone: Tone::Muted,
                            detail: Some(trigger.timing().to_string().into()),
                        },
                    )
                })
                .collect();
            if !triggers.is_empty() {
                let group_id = format!("{id}/g/{}", Group::Triggers.key());
                children.push(self.group(group_id, data_source, schema, Group::Triggers, triggers));
            }
        }
        // Under a filter the matching relation shows, closed.
        let expanded = self.options.expanded.contains(id.as_str());
        TreeItem::new(id, relation.name().to_string())
            .expanded(expanded)
            .children(children)
    }

    fn message(&mut self, parent: &str, failure: Option<SharedString>) -> TreeItem {
        let id = format!("{parent}/message");
        let failed = failure.is_some();
        self.nodes
            .insert(id.clone().into(), Node::Message { failed });
        let label: SharedString = failure.unwrap_or_else(|| t!("explorer.loading").into());
        TreeItem::new(id, label).disabled(true)
    }
}

#[cfg(test)]
mod tests {
    use datakit_catalog::Catalog;
    use datakit_driver::ConnectionProfile;
    use gpui_kit::{AppContext as _, TestAppContext};

    use super::*;

    #[gpui_kit::test]
    fn the_servers_users_and_roles_are_a_folder_of_the_data_source(cx: &mut TestAppContext) {
        let scratch = std::env::temp_dir().join(format!("datakit-tree-{}", std::process::id()));
        cx.update(|cx| {
            crate::services::Services::init_for_test(scratch.clone(), cx).unwrap();
        });
        let data_source = cx.new(|cx| {
            let mut data_source = DataSource::new(ConnectionProfile::new("sqlite", 0), cx);
            data_source.set_catalog(
                Catalog::new("app")
                    .with_schemas([Schema::new("public")])
                    .with_roles([
                        Role::new("admin", true).with_attributes(["SUPERUSER".into()]),
                        Role::new("readers", false),
                    ]),
            );
            data_source
        });
        cx.update(|cx| {
            let expanded = HashSet::new();
            let options = |filter| TreeOptions {
                expanded: &expanded,
                filter,
                show_system_schemas: false,
            };
            let model = build(std::slice::from_ref(&data_source), &options(""), cx);
            let children = &model.items[0].children;
            let roles = children.last().unwrap();
            assert_eq!(roles.label.as_ref(), "Users and roles 2");
            assert!(matches!(
                model.nodes.get(&roles.children[0].id),
                Some(Node::Role { role, .. }) if &*role.name() == "admin"
            ));

            let filtered = build(std::slice::from_ref(&data_source), &options("read"), cx);
            let roles = filtered.items[0].children.last().unwrap();
            assert_eq!(roles.children.len(), 1);
        });
        let _ = std::fs::remove_dir_all(scratch);
    }
}
