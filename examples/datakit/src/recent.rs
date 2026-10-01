//! Recent files: the SQL files and tables opened lately, one keystroke
//! away, as DataGrip's Recent Files list keeps them.

use std::{cell::Cell, path::PathBuf, rc::Rc};

use datakit_driver::DataSourceId;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IndexPath, Sizable as _, WindowExt as _,
    command::{Command, CommandGroup, CommandItem, CommandState},
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, Global,
    ParentElement as _, SharedString, Styled as _, Window, div, px,
};
use rust_i18n::t;
use serde::{Deserialize, Serialize};

use crate::{datasource::DataSources, services::Services};

/// How many entries are kept.
const LIMIT: usize = 30;

/// Something opened lately.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecentEntry {
    File {
        path: PathBuf,
    },
    Table {
        data_source: DataSourceId,
        schema: String,
        relation: String,
    },
}

/// The entries, most recent first, saved to `recent.json`.
#[derive(Default)]
pub struct Recent {
    entries: Vec<RecentEntry>,
}

impl Global for Recent {}

impl Recent {
    pub fn init(cx: &mut App) {
        let entries = std::fs::read(path(cx))
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default();
        cx.set_global(Self { entries });
    }

    pub fn entries(cx: &App) -> &[RecentEntry] {
        &cx.global::<Self>().entries
    }

    /// Put `entry` first.
    pub fn record(entry: RecentEntry, cx: &mut App) {
        let recent = cx.global_mut::<Self>();
        recent.entries.retain(|existing| existing != &entry);
        recent.entries.insert(0, entry);
        recent.entries.truncate(LIMIT);
        let json = serde_json::to_vec_pretty(&recent.entries).unwrap_or_default();
        let path = path(cx);
        cx.background_spawn(async move {
            if let Err(error) = std::fs::write(&path, json) {
                tracing::error!("couldn’t save the recent files: {error}");
            }
        })
        .detach();
    }
}

fn path(cx: &App) -> PathBuf {
    Services::global(cx).data_directory().join("recent.json")
}

pub enum RecentEvent {
    Open(RecentEntry),
}

/// The Recent Files list, filtered as the person types.
pub struct RecentFiles {
    state: Entity<CommandState>,
}

impl EventEmitter<RecentEvent> for RecentFiles {}

impl RecentFiles {
    /// Open the list in a dialog; choosing an entry emits
    /// [`RecentEvent::Open`] from the returned entity.
    pub fn open(window: &mut Window, cx: &mut App) -> Entity<Self> {
        let list = cx.new(|cx| Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
        });
        let dialog_list = list.clone();
        let focus_on_mount = Rc::new(Cell::new(true));
        window.open_dialog(cx, move |dialog, _, _| {
            let list = dialog_list.clone();
            let focus_on_mount = focus_on_mount.clone();
            dialog
                .close_button(false)
                .p_0()
                .w(px(560.))
                .content(move |content, window, cx| {
                    if focus_on_mount.replace(false) {
                        let state = list.read(cx).state.clone();
                        window.defer(cx, move |window, cx| {
                            state.read(cx).focus_handle(cx).focus(window, cx);
                        });
                    }
                    content.child(list.update(cx, |list, cx| list.command(cx)))
                })
        });
        list
    }

    fn confirm(&mut self, index: IndexPath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = Recent::entries(cx).get(index.row).cloned() else {
            return;
        };
        // Before closing: the dialog holds this entity.
        cx.emit(RecentEvent::Open(entry));
        window.close_dialog(cx);
    }

    fn command(&mut self, cx: &mut Context<Self>) -> Command {
        let view = cx.entity().downgrade();
        let items: Vec<CommandItem> = Recent::entries(cx)
            .iter()
            .map(|entry| {
                let (icon, label, detail): (IconName, SharedString, SharedString) = match entry {
                    RecentEntry::File { path } => (
                        IconName::FileCode,
                        path.file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_default()
                            .into(),
                        path.parent()
                            .map(|parent| parent.display().to_string())
                            .unwrap_or_default()
                            .into(),
                    ),
                    RecentEntry::Table {
                        data_source,
                        schema,
                        relation,
                    } => (
                        IconName::Table,
                        relation.clone().into(),
                        DataSources::global(cx)
                            .read(cx)
                            .get(data_source, cx)
                            .map(|source| format!("{schema} · {}", source.read(cx).name()))
                            .unwrap_or_else(|| schema.clone())
                            .into(),
                    ),
                };
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
            })
            .collect();
        let empty = items.is_empty();
        let command = Command::new(&self.state)
            .bordered(false)
            .placeholder(t!("recent.placeholder").to_string())
            .min_h(px(320.))
            .max_h(px(320.))
            .empty(|_, _, cx| {
                v_flex()
                    .w_full()
                    .items_center()
                    .py_6()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("recent.empty").to_string())
            })
            .on_confirm(move |index, window, cx| {
                let _ = view.update(cx, |list, cx| list.confirm(index, window, cx));
            });
        if empty {
            return command;
        }
        command.group(
            CommandGroup::new()
                .label(t!("recent.title").to_string())
                .items(items),
        )
    }
}
