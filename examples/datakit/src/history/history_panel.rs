use std::{collections::HashMap, sync::Arc};

use datakit_driver::DataSourceId;
use datakit_store::{HistoryEntry, HistoryOutcome};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IndexPath, Sizable as _,
    dock::{BasePanel, Panel, PanelEvent},
    h_flex,
    list::{List, ListDelegate, ListEvent, ListItem, ListState},
    menu::{PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window,
    div,
};
use rust_i18n::t;

use super::{HistoryEvent, HistoryLog};
use crate::{datasource::DataSources, format};

/// How many entries the panel lists.
const LIMIT: usize = 500;

#[derive(Clone, Debug, PartialEq)]
pub enum HistoryPanelEvent {
    /// Open `sql` in a console for `data_source`.
    Open {
        data_source: DataSourceId,
        sql: Arc<str>,
    },
}

/// The history tool window.
pub struct HistoryPanel {
    focus_handle: FocusHandle,
    list: Entity<ListState<HistoryList>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for HistoryPanel {}
impl EventEmitter<HistoryPanelEvent> for HistoryPanel {}

impl HistoryPanel {
    pub const NAME: &str = "History";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let list = cx.new(|cx| ListState::new(HistoryList::default(), window, cx).searchable(true));
        let subscriptions = vec![
            cx.subscribe(&HistoryLog::global(cx), |this, _, _: &HistoryEvent, cx| {
                this.reload(cx)
            }),
            cx.subscribe(&list, |this, list, event: &ListEvent, cx| {
                if let ListEvent::Confirm(ix) = event {
                    let entry = list.read(cx).delegate().entries.get(ix.row).cloned();
                    if let Some(entry) = entry {
                        this.open(&entry, cx);
                    }
                }
            }),
        ];
        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            list,
            _subscriptions: subscriptions,
        };
        panel.reload(cx);
        panel
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.list.update(cx, |list, cx| {
            let query = list.delegate().query.clone();
            let task = HistoryList::search(query, cx);
            list.delegate_mut().search_task = Some(task);
        });
    }

    fn open(&mut self, entry: &HistoryEntry, cx: &mut Context<Self>) {
        cx.emit(HistoryPanelEvent::Open {
            data_source: entry.data_source().clone(),
            sql: entry.sql().clone(),
        });
    }
}

/// The entries the panel lists, newest first.
#[derive(Default)]
pub struct HistoryList {
    entries: Vec<HistoryEntry>,
    names: HashMap<DataSourceId, SharedString>,
    query: String,
    selected: Option<IndexPath>,
    search_task: Option<Task<()>>,
}

impl HistoryList {
    fn search(query: String, cx: &mut Context<ListState<Self>>) -> Task<()> {
        let search = HistoryLog::global(cx).read(cx).search(query, LIMIT, cx);
        cx.spawn(async move |list, cx| {
            let entries = match search.await {
                Ok(entries) => entries,
                Err(error) => {
                    tracing::error!("couldn’t search the history: {error:#}");
                    Vec::new()
                }
            };
            let _ = list.update(cx, |list, cx| {
                let names = DataSources::global(cx)
                    .read(cx)
                    .items()
                    .iter()
                    .map(|item| {
                        let item = item.read(cx);
                        (item.profile().id().clone(), item.name())
                    })
                    .collect();
                let delegate = list.delegate_mut();
                delegate.entries = entries;
                delegate.names = names;
                cx.notify();
            });
        })
    }

    fn outcome(entry: &HistoryEntry) -> (IconName, SharedString) {
        match entry.outcome() {
            HistoryOutcome::Rows(rows) => (
                IconName::Check,
                t!("history.rows", count = format::count(*rows as usize)).into(),
            ),
            HistoryOutcome::Command(Some(rows)) => (
                IconName::Check,
                t!("history.affected", count = format::count(*rows as usize)).into(),
            ),
            HistoryOutcome::Command(None) => (IconName::Check, SharedString::default()),
            HistoryOutcome::Failed(error) => (IconName::CircleX, error.to_string().into()),
            HistoryOutcome::Cancelled => (IconName::CircleX, t!("history.cancelled").into()),
        }
    }
}

impl ListDelegate for HistoryList {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_string();
        Self::search(self.query.clone(), cx)
    }

    fn items_count(&self, _: usize, _: &App) -> usize {
        self.entries.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let entry = self.entries.get(ix.row)?;
        let (icon, outcome) = Self::outcome(entry);
        let failed = matches!(
            entry.outcome(),
            HistoryOutcome::Failed(_) | HistoryOutcome::Cancelled
        );
        let sql: SharedString = entry
            .sql()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .into();
        let data_source = self
            .names
            .get(entry.data_source())
            .cloned()
            .unwrap_or_else(|| t!("history.removed_data_source").into());
        let muted = cx.theme().muted_foreground;
        Some(
            ListItem::new(("history", entry.id().unwrap_or_default() as u64))
                .selected(self.selected == Some(ix))
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .text_sm()
                        .child(Icon::new(icon).xsmall().text_color(if failed {
                            cx.theme().danger
                        } else {
                            muted
                        }))
                        .child(
                            div()
                                .flex_none()
                                .text_color(muted)
                                .child(format::date_time(entry.started_at_ms())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(sql),
                        )
                        .child(
                            div()
                                .flex_none()
                                .max_w_64()
                                .truncate()
                                .text_color(muted)
                                .child(outcome),
                        )
                        .child(div().flex_none().text_color(muted).child(format::duration(
                            std::time::Duration::from_millis(entry.duration_ms()),
                        )))
                        .child(
                            div()
                                .flex_none()
                                .max_w_40()
                                .truncate()
                                .text_color(muted)
                                .child(data_source),
                        ),
                ),
        )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .justify_center()
            .items_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(match HistoryLog::global(cx).read(cx).open_error() {
                Some(error) => t!("history.open_failed", error = error).to_string(),
                None if self.query.is_empty() => t!("history.empty").to_string(),
                None => t!("history.no_matches").to_string(),
            })
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
        cx.notify();
    }
}

impl Focusable for HistoryPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for HistoryPanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }

    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for HistoryPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("history.title").to_string()
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let selected = self
            .list
            .read(cx)
            .delegate()
            .selected
            .and_then(|ix| self.list.read(cx).delegate().entries.get(ix.row).cloned());
        menu.item(
            PopupMenuItem::new(t!("history.copy_sql").to_string())
                .disabled(selected.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(entry) = &selected {
                        cx.write_to_clipboard(ClipboardItem::new_string(entry.sql().to_string()));
                    }
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new(t!("history.clear").to_string()).on_click(|_, _, cx| {
                HistoryLog::global(cx).update(cx, |log, cx| log.clear(cx));
            }),
        )
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for HistoryPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(List::new(&self.list).search_placeholder(t!("history.search").to_string()))
    }
}
