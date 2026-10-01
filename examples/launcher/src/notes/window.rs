//! The Floating Notes window: one note at a time, edited in place and saved
//! as it is typed, in a small window that stays above other windows.

use std::{path::PathBuf, time::Duration};

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, Global,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, TitlebarOptions, Window, WindowBounds, WindowKind,
    WindowOptions,
    component::{
        ActiveTheme as _, Icon, IconName, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{InputEvent, Textarea, TextareaState},
        v_flex,
    },
    div, px, size,
};
use serde::{Deserialize, Serialize};

use super::{directory, new_id, read, title_of, write};

/// How long typing pauses before the note is written.
const SAVE_DELAY: Duration = Duration::from_millis(400);

/// Which note the window shows, kept between launches.
#[derive(Default, Deserialize, Serialize)]
struct State {
    current: Option<String>,
}

fn state_path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("notes-window.json"))
}

fn load_state() -> State {
    state_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_state(state: &State) {
    if let (Some(path), Ok(text)) = (state_path(), serde_json::to_string(state)) {
        std::fs::write(path, text).ok();
    }
}

/// The open window, if any.
#[derive(Default)]
struct FloatingNotes {
    window: Option<(AnyWindowHandle, Entity<NotesView>)>,
}

impl Global for FloatingNotes {}

fn open_window(cx: &App) -> Option<(AnyWindowHandle, Entity<NotesView>)> {
    let (handle, view) = cx.try_global::<FloatingNotes>()?.window.clone()?;
    cx.windows().contains(&handle).then_some((handle, view))
}

/// Opens the window on the last note, or closes it when it is open.
pub fn toggle_window(cx: &mut App) {
    crate::shell::launcher::hide(cx);
    cx.defer(|cx| match open_window(cx) {
        Some((handle, view)) => {
            view.update(cx, |view, cx| view.save_now_with(cx));
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
            cx.global_mut::<FloatingNotes>().window = None;
        }
        None => show(load_state().current, cx),
    });
}

/// Shows the note `id` in the window, or a new note when `None`.
pub fn open_note(id: Option<String>, cx: &mut App) {
    crate::shell::launcher::hide(cx);
    let id = Some(id.unwrap_or_else(new_id));
    cx.defer(move |cx| show(id, cx));
}

/// Closes nothing, but stops showing a note that was deleted.
pub fn forget_note(id: &str, cx: &mut App) {
    if let Some((handle, view)) = open_window(cx)
        && view.read(cx).id == id
    {
        handle
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| view.replace(new_id(), window, cx))
            })
            .ok();
    }
}

/// Shows the open note as it is on disk again, after an import replaced it.
pub fn reload_open_note(cx: &mut App) {
    if let Some((handle, view)) = open_window(cx) {
        let id = view.read(cx).id.clone();
        handle
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| view.replace(id, window, cx))
            })
            .ok();
    }
}

fn show(id: Option<String>, cx: &mut App) {
    if let Some((handle, view)) = open_window(cx) {
        handle
            .update(cx, |_, window, cx| {
                if let Some(id) = id {
                    view.update(cx, |view, cx| view.load(id, window, cx));
                }
                window.activate_window();
                focus(&view, window, cx);
            })
            .ok();
        return;
    }
    let id = id
        .or_else(|| {
            directory()
                .and_then(|directory| super::list(&directory).into_iter().next())
                .map(|note| note.id)
        })
        .unwrap_or_else(new_id);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(380.), px(460.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Notes".into()),
            ..Default::default()
        }),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_resizable: true,
        is_minimizable: false,
        app_id: Some("gpui-kit-launcher-notes".into()),
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        keep_on_top(window);
        cx.new(|cx| NotesView::new(id, window, cx))
    });
    match opened {
        Ok((handle, view)) => {
            handle
                .update(cx, |_, window, cx| focus(&view, window, cx))
                .ok();
            cx.default_global::<FloatingNotes>().window = Some((handle, view));
        }
        Err(error) => tracing::error!("cannot open Floating Notes: {error:#}"),
    }
}

fn focus(view: &Entity<NotesView>, window: &mut Window, cx: &mut App) {
    let editor = view.read(cx).editor.clone();
    editor.update(cx, |editor, cx| editor.focus(window, cx));
}

/// Keeps the window above other windows and out of the taskbar, as a
/// floating panel is.
fn keep_on_top(window: &Window) {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{
                GWL_EXSTYLE, GetWindowLongW, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                SetWindowLongW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
            },
        };
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = HWND(handle.hwnd.get() as *mut std::ffi::c_void);
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                ((style & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0) as i32,
            );
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

pub struct NotesView {
    id: String,
    editor: Entity<TextareaState>,
    /// The text last written, so an unchanged note is not written again.
    saved: String,
    save_task: Option<Task<()>>,
    _subscription: Subscription,
}

impl NotesView {
    fn new(id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = directory()
            .and_then(|directory| read(&directory, &id))
            .unwrap_or_default();
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Start typing…")
                .default_value(text.clone())
        });
        let subscription = cx.subscribe_in(&editor, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_save(cx);
                cx.notify();
            }
        });
        save_state(&State {
            current: Some(id.clone()),
        });
        // Closing the window writes typing still waiting for its save.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            this.update(cx, |this, cx| this.save_now_with(cx)).ok();
            true
        });
        Self {
            id,
            editor,
            saved: text,
            save_task: None,
            _subscription: subscription,
        }
    }

    /// Shows the note `id`, after saving the one shown.
    fn load(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.save_now_with(cx);
        self.replace(id, window, cx);
    }

    /// Shows the note `id` as it is on disk, dropping unsaved typing: the
    /// shown note was deleted or replaced by an import.
    fn replace(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.save_task = None;
        let text = directory()
            .and_then(|directory| read(&directory, &id))
            .unwrap_or_default();
        self.editor
            .update(cx, |editor, cx| editor.set_value(text.clone(), window, cx));
        self.saved = text;
        self.id = id;
        save_state(&State {
            current: Some(self.id.clone()),
        });
        cx.notify();
    }

    fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            this.update(cx, |this, cx| this.save_now_with(cx)).ok();
        }));
    }

    /// Writes the note if it changed. An empty new note is not written, so
    /// opening and closing the window leaves no empty files behind.
    fn save_now_with(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        let text = self.text(cx);
        if text == self.saved {
            return;
        }
        let Some(directory) = directory() else {
            return;
        };
        let result = match text.trim().is_empty() {
            true => super::delete(&directory, &self.id).or(Ok(())),
            false => write(&directory, &self.id, &text),
        };
        match result {
            Ok(()) => self.saved = text,
            Err(error) => tracing::warn!("cannot save the note: {error:#}"),
        }
    }

    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load(new_id(), window, cx);
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }
}

impl Render for NotesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let text = self.text(cx);
        let title: SharedString = title_of(&text).into();
        v_flex()
            .size_full()
            .bg(theme.background)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                if keystroke.modifiers.secondary() && keystroke.key == "n" {
                    this.new_note(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                h_flex()
                    .flex_none()
                    .h_9()
                    .px_3()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(title),
                    )
                    .child(
                        Button::new("new-note")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("New Note (Ctrl+N)")
                            .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx))),
                    )
                    .child(
                        Button::new("all-notes")
                            .ghost()
                            .xsmall()
                            .icon(Icon::empty().path("icons/list.svg"))
                            .tooltip("Search Notes")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save_now_with(cx);
                                crate::shell::launcher::open_item("system/search-notes".into(), cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .child(Textarea::new(&self.editor).appearance(false).h_full()),
            )
    }
}
