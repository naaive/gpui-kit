//! Switch Windows: every open window, searchable by its title and its
//! application, to bring to the front, minimize or close.
//!
//! Windows are listed front to back, so the window that was in front before
//! the launcher comes first and switching back and forth is one keystroke.
//! Windows only; elsewhere the command is not offered.

#[cfg(target_os = "windows")]
mod windows;

use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};

use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Effect, Image,
        Item, ItemId, ListModel, PageModel, RunHandler, Section,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
    sources::applications::{REVEAL_TITLE, file_icon},
};

/// One top-level window.
#[derive(Clone, Debug)]
pub struct OpenWindow {
    /// The raw window handle.
    pub handle: isize,
    pub title: String,
    /// The program that owns the window.
    pub exe: Option<PathBuf>,
    pub minimized: bool,
}

impl OpenWindow {
    /// The application's name: the executable's description would be
    /// nicer, but its file name is what people type.
    fn application(&self) -> Option<String> {
        let name = self
            .exe
            .as_ref()?
            .file_stem()?
            .to_string_lossy()
            .into_owned();
        Some(name)
    }
}

/// The executable of the process that owns the window `handle`.
#[cfg(target_os = "windows")]
pub fn window_executable(handle: isize) -> Option<PathBuf> {
    windows::window_executable(handle)
}

/// The application in front, by its executable's name without extension
/// (`code`), as settings name applications.
pub fn frontmost_application() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let exe = windows::window_executable(windows::foreground())?;
        Some(exe.file_stem()?.to_string_lossy().into_owned())
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

pub fn is_supported() -> bool {
    cfg!(target_os = "windows")
}

/// The open windows, front to back. Blocking but fast.
pub fn open_windows() -> Vec<OpenWindow> {
    #[cfg(target_os = "windows")]
    {
        windows::open_windows()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// What can be done to a window.
#[derive(Clone, Copy, Debug)]
enum Command {
    Focus,
    Minimize,
    Close,
}

fn run(command: Command, handle: isize) -> bool {
    #[cfg(target_os = "windows")]
    {
        match command {
            Command::Focus => windows::focus(handle),
            Command::Minimize => windows::minimize(handle),
            Command::Close => windows::close(handle),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (command, handle);
        false
    }
}

pub fn switch_windows_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|cx| {
        let mut page = SwitchWindowsPage {
            windows: Vec::new(),
            icons: HashMap::new(),
            loaded: false,
            task: None,
        };
        page.load(cx);
        page
    })))
}

struct SwitchWindowsPage {
    windows: Vec<OpenWindow>,
    icons: HashMap<PathBuf, Option<PathBuf>>,
    loaded: bool,
    task: Option<Task<()>>,
}

/// Icons by executable, shared by every time the page opens.
fn icon_cache() -> &'static Mutex<HashMap<PathBuf, Option<PathBuf>>> {
    static ICONS: std::sync::OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
        std::sync::OnceLock::new();
    ICONS.get_or_init(Default::default)
}

impl SwitchWindowsPage {
    /// Lists windows at once, then looks up icons in the background.
    fn load(&mut self, cx: &mut Context<Self>) {
        self.windows = open_windows();
        self.loaded = true;
        if let Ok(icons) = icon_cache().lock() {
            self.icons = icons.clone();
        }
        let missing: Vec<PathBuf> = self
            .windows
            .iter()
            .filter_map(|window| window.exe.clone())
            .filter(|exe| !self.icons.contains_key(exe))
            .collect();
        if missing.is_empty() {
            return;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let mut found = HashMap::new();
                    for exe in missing {
                        let icon = file_icon(&exe);
                        found.insert(exe, icon);
                    }
                    found
                })
                .await;
            if let Ok(mut icons) = icon_cache().lock() {
                icons.extend(found.clone());
            }
            this.update(cx, |page, cx| {
                page.icons.extend(found);
                cx.notify();
            })
            .ok();
        }));
    }

    fn item(&self, window: &OpenWindow, cx: &Context<Self>) -> Item {
        let handle = window.handle;
        let application = window.application();
        let image = window
            .exe
            .as_ref()
            .and_then(|exe| self.icons.get(exe).cloned().flatten())
            .map(Image::File)
            .unwrap_or(Image::Icon("app-window".into()));
        let page = cx.entity().downgrade();
        let after = move |command: Command| {
            let page = page.clone();
            Effect::Run(RunHandler::new(move |(), _, cx| {
                if !run(command, handle) {
                    perform(
                        Effect::ShowToast(crate::model::Toast::new(
                            crate::model::ToastStyle::Failure,
                            "Couldn’t reach the window",
                        )),
                        cx,
                    );
                }
                // The list changes: a closed window is gone, a minimized one
                // is marked.
                let page = page.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(250))
                        .await;
                    page.update(cx, |page, cx| page.load(cx)).ok();
                })
                .detach();
            }))
        };
        let mut actions = ActionPanel::new()
            .with_action(
                Action::new(
                    "Switch to Window",
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        // Brought to the front while the launcher still is,
                        // which Windows allows; the launcher then hides
                        // because it is no longer active.
                        if run(Command::Focus, handle) {
                            crate::shell::launcher::hide(cx);
                        }
                    })),
                )
                .with_image(Image::Icon("app-window".into())),
            )
            .with_action(
                Action::new("Minimize Window", after(Command::Minimize))
                    .with_image(Image::Icon("minimize".into()))
                    .with_shortcut("secondary-m"),
            )
            .with_action(
                Action::new("Close Window", after(Command::Close))
                    .with_image(Image::Icon("x".into()))
                    .with_style(ActionStyle::Destructive)
                    .with_shortcut("secondary-w"),
            )
            .with_action(
                Action::new("Copy Title", Effect::Copy(window.title.clone().into()))
                    .with_image(Image::Icon("copy".into()))
                    .with_shortcut("secondary-shift-c"),
            );
        if let Some(exe) = &window.exe {
            actions = actions.with_section(
                ActionSection::new().with_entry(ActionEntry::Action(
                    Action::new(REVEAL_TITLE, Effect::RevealPath(exe.clone()))
                        .with_image(Image::Icon("folder-open".into()))
                        .with_shortcut("secondary-shift-f"),
                )),
            );
        }
        let mut item = Item::new(
            ItemId::new(format!("window-{handle}")),
            window.title.clone(),
        )
        .with_image(image)
        .with_actions(actions);
        if let Some(application) = application {
            item = item
                .with_subtitle(application.clone())
                .with_keyword(application);
        }
        if window.minimized {
            item = item.with_accessory(
                Accessory::image(Image::Icon("minimize".into())).with_tooltip("Minimized"),
            );
        }
        item
    }
}

impl Page for SwitchWindowsPage {
    fn title(&self) -> SharedString {
        "Switch Windows".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        ListModel::new()
            .with_placeholder("Search open windows…")
            .with_loading(!self.loaded)
            .with_empty_title("No open windows")
            .with_section(
                Section::new()
                    .with_title("Windows")
                    .with_subtitle(self.windows.len().to_string())
                    .with_items(self.windows.iter().map(|window| self.item(window, cx))),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}

    fn did_reappear(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
        cx.notify();
    }
}
