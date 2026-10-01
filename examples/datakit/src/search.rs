//! Search Everywhere: one field that finds any object of any data source by
//! name, and any command by what it does.

use std::{cell::Cell, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IndexPath, Sizable as _, WindowExt as _,
    command::{Command, CommandGroup, CommandItem, CommandState},
    h_flex, v_flex,
};
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, EventEmitter, Focusable as _,
    ParentElement as _, SharedString, Styled as _, Window, div, px,
};
use rust_i18n::t;

use crate::{
    datasource::DataSources,
    objects::{self, ObjectPath, ObjectRef},
};

/// How many objects a query shows at most.
const LIMIT: usize = 80;

/// What Search Everywhere looks for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    Everything,
    /// Only objects: Go to object.
    Objects,
    /// Only commands: Find action.
    Actions,
}

pub enum SearchEvent {
    Open(ObjectRef),
}

/// A command Search Everywhere can run.
pub struct SearchableAction {
    pub label: SharedString,
    pub action: Box<dyn Action>,
}

struct ObjectMatch {
    object: ObjectRef,
    label: SharedString,
    detail: SharedString,
    icon: IconName,
    score: u32,
}

pub struct SearchEverywhere {
    state: Entity<CommandState>,
    scope: SearchScope,
    actions: Vec<SearchableAction>,
    objects: Vec<ObjectMatch>,
    matched_actions: Vec<usize>,
}

impl EventEmitter<SearchEvent> for SearchEverywhere {}

impl SearchEverywhere {
    pub fn new(
        scope: SearchScope,
        actions: Vec<SearchableAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.new(|cx| CommandState::new(window, cx));
        let mut search = Self {
            state,
            scope,
            actions,
            objects: Vec::new(),
            matched_actions: Vec::new(),
        };
        search.query("", cx);
        search
    }

    /// Open Search Everywhere in a dialog; choosing an object emits
    /// [`SearchEvent::Open`] from the returned entity.
    pub fn open(
        scope: SearchScope,
        actions: Vec<SearchableAction>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let search = cx.new(|cx| Self::new(scope, actions, window, cx));
        let dialog_search = search.clone();
        let focus_on_mount = Rc::new(Cell::new(true));
        window.open_dialog(cx, move |dialog, _, _| {
            let search = dialog_search.clone();
            let focus_on_mount = focus_on_mount.clone();
            dialog
                .close_button(false)
                .p_0()
                .w(px(640.))
                .content(move |content, window, cx| {
                    if focus_on_mount.replace(false) {
                        let state = search.read(cx).state.clone();
                        window.defer(cx, move |window, cx| {
                            state.read(cx).focus_handle(cx).focus(window, cx);
                        });
                    }
                    content.child(search.update(cx, |search, cx| search.command(cx)))
                })
        });
        search
    }

    fn placeholder(&self) -> SharedString {
        match self.scope {
            SearchScope::Everything => t!("search.placeholder"),
            SearchScope::Objects => t!("search.placeholder_objects"),
            SearchScope::Actions => t!("search.placeholder_actions"),
        }
        .into()
    }

    fn query(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.trim().to_lowercase();
        self.objects = if self.scope == SearchScope::Actions || query.is_empty() {
            Vec::new()
        } else {
            find_objects(&query, cx)
        };
        self.matched_actions = if self.scope == SearchScope::Objects {
            Vec::new()
        } else {
            let mut scored: Vec<(u32, usize)> = self
                .actions
                .iter()
                .enumerate()
                .filter_map(|(ix, action)| {
                    score(&action.label.to_lowercase(), &query).map(|score| (score, ix))
                })
                .collect();
            scored.sort();
            scored.into_iter().map(|(_, ix)| ix).collect()
        };
        cx.notify();
    }

    fn confirm(&mut self, index: IndexPath, window: &mut Window, cx: &mut Context<Self>) {
        // Objects are the first group when there are any.
        let objects_group = (!self.objects.is_empty()) as usize;
        if objects_group == 1 && index.section == 0 {
            if let Some(found) = self.objects.get(index.row) {
                // Before closing: the dialog holds this entity.
                cx.emit(SearchEvent::Open(found.object.clone()));
                window.close_dialog(cx);
            }
            return;
        }
        let Some(action) = self
            .matched_actions
            .get(index.row)
            .and_then(|ix| self.actions.get(*ix))
            .map(|action| action.action.boxed_clone())
        else {
            return;
        };
        // Closing gives the focus back to where the person was, which is
        // where the command applies.
        window.close_dialog(cx);
        window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
    }

    fn command(&mut self, cx: &mut Context<Self>) -> Command {
        let view = cx.entity().downgrade();
        let confirm = view.clone();
        let mut command = Command::new(&self.state)
            .bordered(false)
            .filterable(false)
            .placeholder(self.placeholder())
            .min_h(px(360.))
            .max_h(px(360.))
            .empty(|state, _, cx| {
                let text: SharedString = if state.query(cx).trim().is_empty() {
                    t!("search.hint").into()
                } else {
                    t!("search.nothing_found").into()
                };
                v_flex()
                    .w_full()
                    .items_center()
                    .py_6()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(text)
            })
            .on_query(move |query, _, cx| {
                let _ = view.update(cx, |search, cx| search.query(query, cx));
            })
            .on_confirm(move |index, window, cx| {
                let _ = confirm.update(cx, |search, cx| search.confirm(index, window, cx));
            });
        if !self.objects.is_empty() {
            let items = self.objects.iter().map(|found| {
                let label = found.label.clone();
                let detail = found.detail.clone();
                let icon = found.icon;
                CommandItem::new().label(label.clone()).child(move |_, cx| {
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .text_sm()
                        .child(
                            Icon::new(icon)
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(div().flex_none().truncate().child(label.clone()))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(detail.clone()),
                        )
                })
            });
            command = command.group(
                CommandGroup::new()
                    .label(t!("search.objects").to_string())
                    .items(items),
            );
        }
        if !self.matched_actions.is_empty() {
            let items = self.matched_actions.iter().map(|ix| {
                CommandItem::new()
                    .label(self.actions[*ix].label.clone())
                    .icon(Icon::new(IconName::SquareTerminal))
            });
            command = command.group(
                CommandGroup::new()
                    .label(t!("search.actions").to_string())
                    .items(items),
            );
        }
        command
    }
}

/// How well `name` matches `query`, lower is better; `None` when it does not.
fn score(name: &str, query: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    if name == query {
        return Some(0);
    }
    if name.starts_with(query) {
        return Some(1);
    }
    if name
        .match_indices(query)
        .any(|(ix, _)| ix > 0 && !name.as_bytes()[ix - 1].is_ascii_alphanumeric())
    {
        return Some(2);
    }
    if name.contains(query) {
        return Some(3);
    }
    // Every character in order: `ordit` finds `order_items`.
    let mut rest = name.chars();
    query.chars().all(|c| rest.any(|n| n == c)).then_some(4)
}

fn find_objects(query: &str, cx: &App) -> Vec<ObjectMatch> {
    // `schema.name` narrows by schema.
    let (schema_query, name_query) = match query.split_once('.') {
        Some((schema, name)) => (Some(schema), name),
        None => (None, query),
    };
    let mut found = Vec::new();
    for data_source in DataSources::global(cx).read(cx).items() {
        let source = data_source.read(cx);
        let data_source_name = source.name();
        for schema in source.catalog().schemas() {
            if schema.is_system() {
                continue;
            }
            if let Some(schema_query) = schema_query
                && score(&schema.name().to_lowercase(), schema_query).is_none()
            {
                continue;
            }
            let detail: SharedString = format!("{} · {}", schema.name(), data_source_name).into();
            let mut push = |path: ObjectPath, name: &str, icon: IconName, rank: u32| {
                if let Some(score) = score(&name.to_lowercase(), name_query) {
                    found.push(ObjectMatch {
                        object: ObjectRef::new(data_source.clone(), path),
                        label: name.to_string().into(),
                        detail: detail.clone(),
                        icon,
                        score: score * 4 + rank,
                    });
                }
            };
            for relation in schema.relations().unwrap_or_default() {
                push(
                    ObjectPath::relation(schema.name(), relation.name()),
                    &relation.name(),
                    objects::relation_icon(relation.relation_type()),
                    0,
                );
            }
            for routine in schema.routines() {
                push(
                    ObjectPath::Routine {
                        schema: schema.name(),
                        signature: routine.signature().into(),
                    },
                    &routine.name(),
                    IconName::SquareFunction,
                    1,
                );
            }
            for sequence in schema.sequences() {
                push(
                    ObjectPath::Sequence {
                        schema: schema.name(),
                        sequence: sequence.name(),
                    },
                    &sequence.name(),
                    IconName::Hash,
                    2,
                );
            }
        }
    }
    found.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.label.cmp(&b.label)));
    found.truncate(LIMIT);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_prefix_matches_rank_first() {
        assert_eq!(score("orders", "orders"), Some(0));
        assert_eq!(score("orders", "ord"), Some(1));
        assert_eq!(score("order_items", "items"), Some(2));
        assert_eq!(score("big_orders", "rder"), Some(3));
        assert_eq!(score("order_items", "ordit"), Some(4));
        assert_eq!(score("orders", "xyz"), None);
    }
}
