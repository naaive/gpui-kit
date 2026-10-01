//! Local History: earlier versions of a console's text, kept in the data
//! directory whatever happens to the console or its file, to look at and go
//! back to.

use std::path::{Path, PathBuf};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Editor, EditorState},
    list::ListItem,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Task, Window, div, px, rems,
};
use rust_i18n::t;

use crate::{format, services::Services};

/// How many versions a console keeps.
const KEEP: usize = 100;
/// How long after a version the next one is kept, unless the text ran.
const INTERVAL_MS: i64 = 2 * 60 * 1000;

/// One kept version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    time_ms: i64,
    path: PathBuf,
}

impl Version {
    pub fn time_ms(&self) -> i64 {
        self.time_ms
    }

    pub fn text(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }
}

/// Where the versions of the text known as `key` — a file's path, a scratch
/// console's id — are kept.
fn directory(key: &str, cx: &App) -> PathBuf {
    // FNV-1a: a name that stays the same from one build to the next.
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3)
    });
    Services::global(cx)
        .data_directory()
        .join("local-history")
        .join(format!("{hash:016x}"))
}

/// Keep `text` as a version of `key`, unless it is the latest version, or
/// the latest is recent and `ran` is false: text that runs is always kept.
pub fn record(key: &str, text: String, ran: bool, cx: &App) -> Task<()> {
    let directory = directory(key, cx);
    let now = format::now_ms();
    cx.background_spawn(async move {
        if text.trim().is_empty() {
            return;
        }
        let versions = list(&directory);
        if let Some(latest) = versions.first() {
            if latest.text() == text || (!ran && now - latest.time_ms < INTERVAL_MS) {
                return;
            }
        }
        let written = std::fs::create_dir_all(&directory)
            .and_then(|()| std::fs::write(directory.join(format!("{now}.sql")), &text));
        if let Err(error) = written {
            tracing::error!("couldn’t keep a local history version: {error}");
            return;
        }
        for old in versions.iter().skip(KEEP - 1) {
            let _ = std::fs::remove_file(&old.path);
        }
    })
}

/// The versions of `key`, newest first.
pub fn versions(key: &str, cx: &App) -> Vec<Version> {
    list(&directory(key, cx))
}

fn list(directory: &Path) -> Vec<Version> {
    let mut versions: Vec<Version> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let time_ms = path.file_stem()?.to_str()?.parse().ok()?;
            Some(Version { time_ms, path })
        })
        .collect();
    versions.sort_by_key(|version| std::cmp::Reverse(version.time_ms));
    versions
}

/// The versions in a dialog: pick one to see it, and revert to it.
pub struct LocalHistoryView {
    versions: Vec<Version>,
    selected: usize,
    preview: Entity<EditorState>,
}

impl LocalHistoryView {
    /// Show the versions of `key`; reverting hands the chosen text to
    /// `revert`.
    pub fn open(
        key: &str,
        title: SharedString,
        revert: impl Fn(String, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) {
        let versions = versions(key, cx);
        let view = cx.new(|cx| {
            let preview = cx.new(|cx| {
                EditorState::new(window, cx)
                    .language("sql")
                    .line_number(true)
            });
            let mut view = Self {
                versions,
                selected: 0,
                preview,
            };
            view.show(0, window, cx);
            view
        });
        let width = rems(52.).to_pixels(window.rem_size());
        let revert = std::rc::Rc::new(revert);
        window.open_dialog(cx, move |dialog, _, cx| {
            let empty = view.read(cx).versions.is_empty();
            let chosen = view.clone();
            let revert = revert.clone();
            dialog
                .title(t!("local_history.title", name = title).to_string())
                .w(width)
                .child(view.clone())
                .footer(
                    DialogFooter::new().child(
                        h_flex()
                            .gap_2()
                            .child(
                                DialogClose::new().child(
                                    Button::new("close")
                                        .outline()
                                        .label(t!("common.cancel").to_string()),
                                ),
                            )
                            .child(
                                DialogAction::new().child(
                                    Button::new("revert")
                                        .primary()
                                        .disabled(empty)
                                        .label(t!("local_history.revert").to_string()),
                                ),
                            ),
                    ),
                )
                .on_ok(move |_, window, cx| {
                    let text = chosen.read(cx).preview.read(cx).value().to_string();
                    if !chosen.read(cx).versions.is_empty() {
                        revert(text, window, cx);
                    }
                    true
                })
        });
    }

    fn show(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = ix;
        let text = self.versions.get(ix).map(Version::text).unwrap_or_default();
        self.preview
            .update(cx, |preview, cx| preview.set_value(text, window, cx));
        cx.notify();
    }
}

impl Render for LocalHistoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        if self.versions.is_empty() {
            return div()
                .py_6()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t!("local_history.empty").to_string())
                .into_any_element();
        }
        h_flex()
            .h(px(360.))
            .gap_2()
            .child(
                v_flex()
                    .id("versions")
                    .w(rems(12.))
                    .flex_none()
                    .h_full()
                    .overflow_y_scrollbar()
                    .children(self.versions.iter().enumerate().map(|(ix, version)| {
                        ListItem::new(ix)
                            .selected(ix == self.selected)
                            .py_1()
                            .text_sm()
                            .child(format::date_time(version.time_ms()))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.show(ix, window, cx)),
                            )
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .child(
                        Editor::new(&self.preview)
                            .bordered(false)
                            .disabled(true)
                            .h_full()
                            .text_size(theme.mono_font_size)
                            .font_family(theme.mono_font_family.clone()),
                    ),
            )
            .into_any_element()
    }
}
