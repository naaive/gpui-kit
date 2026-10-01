//! Find Usages: where the open consoles name a table, view, column or
//! routine, as a list to go through.

use std::{cell::Cell, ops::Range, rc::Rc};

use datakit_sql::{Target, usages};
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, WindowExt as _,
    command::{Command, CommandGroup, CommandItem, CommandState},
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, ParentElement as _, SharedString,
    Styled as _, WeakEntity, Window, div, px,
};
use rust_i18n::t;

use super::{ConsolePanel, Sessions};
use crate::{
    datasource::DataSource,
    navigation::{Navigation, NavigationEvent},
};

/// One name that refers to the object.
struct Usage {
    console: WeakEntity<ConsolePanel>,
    console_name: SharedString,
    range: Range<usize>,
    /// 1-based.
    line: usize,
    line_text: SharedString,
}

/// The usages, in a dialog.
pub struct UsagesList {
    state: Entity<CommandState>,
    title: SharedString,
    usages: Vec<Usage>,
}

impl UsagesList {
    /// Find the usages of `target` in the consoles on `data_source` and
    /// show them; one usage is gone to straight away.
    pub fn open(
        name: &str,
        target: &Target,
        data_source: &Entity<DataSource>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let found = find(target, data_source, cx);
        if let [usage] = found.as_slice() {
            show(usage, cx);
            return;
        }
        let list = cx.new(|cx| Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
            title: t!("usages.title", name = name, count = found.len()).into(),
            usages: found,
        });
        let focus_on_mount = Rc::new(Cell::new(true));
        window.open_dialog(cx, move |dialog, _, _| {
            let list = list.clone();
            let focus_on_mount = focus_on_mount.clone();
            dialog
                .close_button(false)
                .p_0()
                .w(px(640.))
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
    }

    fn confirm(&mut self, index: IndexPath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(usage) = self.usages.get(index.row) else {
            return;
        };
        let (console, range) = (usage.console.clone(), usage.range.clone());
        window.close_dialog(cx);
        if let Some(console) = console.upgrade() {
            Navigation::request(NavigationEvent::ShowConsole { console, range }, cx);
        }
    }

    fn command(&mut self, cx: &mut Context<Self>) -> Command {
        let view = cx.entity().downgrade();
        let items: Vec<CommandItem> = self
            .usages
            .iter()
            .map(|usage| {
                let text = usage.line_text.clone();
                let place: SharedString = format!("{}:{}", usage.console_name, usage.line).into();
                CommandItem::new()
                    .label(format!("{place} {text}"))
                    .child(move |_, cx| {
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .text_sm()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .child(text.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(place.clone()),
                            )
                    })
            })
            .collect();
        let command = Command::new(&self.state)
            .bordered(false)
            .placeholder(t!("usages.placeholder").to_string())
            .min_h(px(320.))
            .max_h(px(320.))
            .empty(|_, _, cx| {
                v_flex()
                    .w_full()
                    .items_center()
                    .py_6()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("usages.none").to_string())
            })
            .on_confirm(move |index, window, cx| {
                let _ = view.update(cx, |list, cx| list.confirm(index, window, cx));
            });
        if items.is_empty() {
            return command;
        }
        command.group(CommandGroup::new().label(self.title.clone()).items(items))
    }
}

fn find(target: &Target, data_source: &Entity<DataSource>, cx: &App) -> Vec<Usage> {
    let source = data_source.read(cx);
    let catalog = source.catalog();
    let dialect = source.dialect();
    let mut found = Vec::new();
    for console in Sessions::global(cx).read(cx).consoles() {
        let panel = console.read(cx);
        if panel.data_source() != Some(data_source) {
            continue;
        }
        let text = panel.text(cx);
        for range in usages(&text, target, catalog, &*dialect) {
            let line_start = text[..range.start].rfind('\n').map_or(0, |ix| ix + 1);
            let line_end = text[range.end..]
                .find('\n')
                .map_or(text.len(), |ix| range.end + ix);
            found.push(Usage {
                console: console.downgrade(),
                console_name: panel.name(),
                line: text[..range.start].matches('\n').count() + 1,
                line_text: text[line_start..line_end].trim().to_string().into(),
                range,
            });
        }
    }
    found
}

fn show(usage: &Usage, cx: &mut App) {
    if let Some(console) = usage.console.upgrade() {
        Navigation::request(
            NavigationEvent::ShowConsole {
                console,
                range: usage.range.clone(),
            },
            cx,
        );
    }
}
