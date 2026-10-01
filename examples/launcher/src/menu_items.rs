//! Search Menu Items: the menu commands of the window that was in front
//! before the launcher, to run by name. Windows only; elsewhere the command
//! is not offered.
//!
//! A classic window's menu bar is read whole, submenus included, and its
//! commands run as the menu would run them. A window that draws its own menu
//! bar exposes it through UI Automation instead; only its top-level menus
//! are listed, since reading a submenu would mean opening it, and choosing
//! one opens that menu in the application.

use std::time::Duration;

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};

use crate::{
    model::{
        Accessory, Action, Effect, Image, Item, ItemId, ListModel, PageModel, RunHandler, Section,
    },
    pages::{self, Page, PageHandle},
};

/// One menu command.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    /// The menus it is in, outermost first, such as `["File", "Export"]`.
    pub path: Vec<String>,
    pub title: String,
    /// The shortcut the menu shows for it, such as `Ctrl+S`.
    pub shortcut: Option<String>,
    pub enabled: bool,
    pub checked: bool,
    pub target: Target,
}

/// How a menu command is run.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A classic menu's command id, sent to the window.
    Command { window: isize, id: u32 },
    /// A drawn menu bar's top-level menu, opened through UI Automation.
    Automation { window: isize, index: usize },
}

/// The window's title and its menu commands. Blocking.
fn read(window: isize) -> (String, Vec<MenuItem>) {
    #[cfg(target_os = "windows")]
    {
        windows::read(window)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        (String::new(), Vec::new())
    }
}

/// Runs `target` in its window. Blocking.
fn run(target: &Target) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::run(target)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = target;
        false
    }
}

pub fn is_supported() -> bool {
    cfg!(target_os = "windows")
}

/// Strips a menu label's access-key ampersands (`&&` is a literal one) and
/// splits off the shortcut the menu shows after a tab.
fn clean_label(label: &str) -> (String, Option<String>) {
    let (text, shortcut) = match label.split_once('\t') {
        Some((text, shortcut)) => (text, Some(shortcut.trim().to_owned())),
        None => (label, None),
    };
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                cleaned.push('&');
                chars.next();
            }
            continue;
        }
        cleaned.push(c);
    }
    (
        cleaned.trim().to_owned(),
        shortcut.filter(|shortcut| !shortcut.is_empty()),
    )
}

pub fn search_menu_items_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let window = crate::window_layout::frontmost();
    Ok(pages::handle(cx.new(|cx| {
        let task = cx.spawn(async move |this, cx| {
            let (title, items) = match window {
                Some(window) => cx.background_spawn(async move { read(window) }).await,
                None => (String::new(), Vec::new()),
            };
            this.update(cx, |page: &mut MenuItemsPage, cx| {
                page.window_title = title;
                page.items = Some(items);
                cx.notify();
            })
            .ok();
        });
        MenuItemsPage {
            window_title: String::new(),
            items: None,
            _task: task,
        }
    })))
}

struct MenuItemsPage {
    window_title: String,
    items: Option<Vec<MenuItem>>,
    _task: Task<()>,
}

fn item(ix: usize, menu_item: &MenuItem) -> Item {
    let target = menu_item.target.clone();
    let title = match &menu_item.target {
        Target::Automation { .. } => "Open Menu",
        Target::Command { .. } => "Run Menu Command",
    };
    let mut item = Item::new(ItemId::new(format!("menu/{ix}")), menu_item.title.clone())
        .with_subtitle(menu_item.path.join(" › "))
        .with_image(Image::Icon(match menu_item.checked {
            true => "check".into(),
            false => "square-menu".into(),
        }))
        .with_keyword(menu_item.path.join(" "))
        .with_action(
            Action::new(
                title,
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    let target = target.clone();
                    crate::shell::launcher::hide(cx);
                    let executor = cx.background_executor().clone();
                    cx.spawn(async move |cx| {
                        // The launcher has to be gone, so the window can
                        // take the front again.
                        executor.timer(Duration::from_millis(120)).await;
                        let ran = cx.background_spawn(async move { run(&target) }).await;
                        if !ran {
                            cx.update(|cx| {
                                crate::shell::platform::show_hud(
                                    "Couldn’t run the menu command".into(),
                                    cx,
                                )
                            });
                        }
                    })
                    .detach();
                })),
            )
            .with_image(Image::Icon("play".into())),
        );
    if let Some(shortcut) = &menu_item.shortcut {
        item = item.with_accessory(Accessory::text(shortcut.clone()));
    }
    if !menu_item.enabled {
        item = item.with_accessory(Accessory::text("Unavailable"));
    }
    item
}

impl Page for MenuItemsPage {
    fn title(&self) -> SharedString {
        "Menu Items".into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let items = self.items.as_deref().unwrap_or_default();
        // Commands that cannot run now go last.
        let (enabled, disabled): (Vec<_>, Vec<_>) =
            items.iter().enumerate().partition(|(_, item)| item.enabled);
        let section_title = match self.window_title.is_empty() {
            true => "Menu Items".to_owned(),
            false => self.window_title.clone(),
        };
        ListModel::new()
            .with_placeholder("Search menu items…")
            .with_loading(self.items.is_none())
            .with_empty_title(match self.items {
                None => "Reading menus…",
                Some(_) => "No menu items",
            })
            .with_empty_description(
                "The window in front before the launcher has no menu bar it exposes.",
            )
            .with_section(
                Section::new()
                    .with_title(section_title)
                    .with_items(enabled.into_iter().map(|(ix, menu)| item(ix, menu))),
            )
            .with_section(
                Section::new()
                    .with_title("Unavailable")
                    .with_items(disabled.into_iter().map(|(ix, menu)| item(ix, menu))),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::c_void;

    use windows::{
        Win32::{
            Foundation::{HWND, LPARAM, WPARAM},
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            },
            UI::{
                Accessibility::{
                    CUIAutomation, IUIAutomation, IUIAutomationElement,
                    IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
                    TreeScope_Children, TreeScope_Descendants, UIA_ControlTypePropertyId,
                    UIA_ExpandCollapsePatternId, UIA_InvokePatternId, UIA_MenuBarControlTypeId,
                    UIA_MenuItemControlTypeId,
                },
                WindowsAndMessaging::{
                    GetMenu, GetMenuItemCount, GetMenuItemInfoW, GetWindowTextLengthW,
                    GetWindowTextW, HMENU, IsWindow, MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED,
                    MFT_OWNERDRAW, MFT_SEPARATOR, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING,
                    MIIM_SUBMENU, PostMessageW, WM_COMMAND,
                },
            },
        },
        core::{PWSTR, VARIANT},
    };

    use super::{MenuItem, Target, clean_label};

    /// Menus nest no deeper than this; a cycle cannot recurse forever.
    const MAX_DEPTH: usize = 8;

    fn hwnd(handle: isize) -> HWND {
        HWND(handle as *mut c_void)
    }

    fn window_title(window: HWND) -> String {
        unsafe {
            let length = GetWindowTextLengthW(window);
            let mut buffer = vec![0u16; length.max(0) as usize + 1];
            let copied = GetWindowTextW(window, &mut buffer);
            String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
        }
    }

    pub fn read(handle: isize) -> (String, Vec<MenuItem>) {
        let window = hwnd(handle);
        if !unsafe { IsWindow(window) }.as_bool() {
            return (String::new(), Vec::new());
        }
        let title = window_title(window);
        let menu = unsafe { GetMenu(window) };
        let mut items = Vec::new();
        if !menu.is_invalid() {
            read_menu(handle, menu, &mut Vec::new(), &mut items, 0);
        }
        if items.is_empty() {
            items = automation_menus(handle);
        }
        (title, items)
    }

    /// One entry of a classic menu: its label, id, submenu and state;
    /// `None` for a separator or an entry the application draws itself.
    fn entry(menu: HMENU, position: u32) -> Option<(String, u32, HMENU, u32)> {
        unsafe {
            // Without a buffer, the call gives the label's length.
            let mut info = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING | MIIM_ID | MIIM_SUBMENU | MIIM_FTYPE | MIIM_STATE,
                ..Default::default()
            };
            GetMenuItemInfoW(menu, position, true, &mut info).ok()?;
            if info.fType.0 & (MFT_SEPARATOR.0 | MFT_OWNERDRAW.0) != 0 || info.cch == 0 {
                return None;
            }
            let mut buffer = vec![0u16; info.cch as usize + 1];
            let mut label = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING,
                dwTypeData: PWSTR(buffer.as_mut_ptr()),
                cch: info.cch + 1,
                ..Default::default()
            };
            GetMenuItemInfoW(menu, position, true, &mut label).ok()?;
            let label = String::from_utf16_lossy(&buffer[..label.cch as usize]);
            Some((label, info.wID, info.hSubMenu, info.fState.0))
        }
    }

    fn read_menu(
        window: isize,
        menu: HMENU,
        path: &mut Vec<String>,
        items: &mut Vec<MenuItem>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let count = unsafe { GetMenuItemCount(menu) };
        for position in 0..count.max(0) as u32 {
            let Some((label, id, submenu, state)) = entry(menu, position) else {
                continue;
            };
            let (title, shortcut) = clean_label(&label);
            if title.is_empty() {
                continue;
            }
            let enabled = state & MFS_DISABLED.0 == 0;
            let checked = state & MFS_CHECKED.0 != 0;
            if !submenu.is_invalid() {
                path.push(title);
                read_menu(window, submenu, path, items, depth + 1);
                path.pop();
            } else {
                items.push(MenuItem {
                    path: path.clone(),
                    title,
                    shortcut,
                    enabled,
                    checked,
                    target: Target::Command { window, id },
                });
            }
        }
    }

    fn automation() -> Option<IUIAutomation> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()
        }
    }

    /// The top-level items of the window's drawn menu bar, less the title
    /// bar's system menu.
    fn menu_bar_items(automation: &IUIAutomation, window: isize) -> Vec<IUIAutomationElement> {
        unsafe {
            let Ok(root) = automation.ElementFromHandle(hwnd(window)) else {
                return Vec::new();
            };
            let condition = |kind: i32| {
                automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(kind))
            };
            let (Ok(bar), Ok(item)) = (
                condition(UIA_MenuBarControlTypeId.0),
                condition(UIA_MenuItemControlTypeId.0),
            ) else {
                return Vec::new();
            };
            let Ok(bars) = root.FindAll(TreeScope_Descendants, &bar) else {
                return Vec::new();
            };
            for ix in 0..bars.Length().unwrap_or(0) {
                let Ok(bar) = bars.GetElement(ix) else {
                    continue;
                };
                let Ok(items) = bar.FindAll(TreeScope_Children, &item) else {
                    continue;
                };
                let items: Vec<IUIAutomationElement> = (0..items.Length().unwrap_or(0))
                    .filter_map(|ix| items.GetElement(ix).ok())
                    .collect();
                // The system menu bar holds just the window's System item.
                let is_system = items.len() <= 1
                    && items.iter().all(|item| {
                        item.CurrentName()
                            .is_ok_and(|name| name.to_string().eq_ignore_ascii_case("system"))
                    });
                if !items.is_empty() && !is_system {
                    return items;
                }
            }
            Vec::new()
        }
    }

    fn automation_menus(window: isize) -> Vec<MenuItem> {
        let Some(automation) = automation() else {
            return Vec::new();
        };
        menu_bar_items(&automation, window)
            .iter()
            .enumerate()
            .filter_map(|(index, element)| {
                let name = unsafe { element.CurrentName() }.ok()?.to_string();
                let (title, shortcut) = clean_label(&name);
                let enabled = unsafe { element.CurrentIsEnabled() }.is_ok_and(|on| on.as_bool());
                (!title.is_empty()).then(|| MenuItem {
                    path: Vec::new(),
                    title,
                    shortcut,
                    enabled,
                    checked: false,
                    target: Target::Automation { window, index },
                })
            })
            .collect()
    }

    /// Sends the menu command `id` to `window`, as choosing it in the
    /// menu does: `WM_COMMAND` with the id and no control.
    fn post_command(window: isize, id: u32) -> bool {
        unsafe { PostMessageW(hwnd(window), WM_COMMAND, WPARAM(id as usize), LPARAM(0)).is_ok() }
    }

    pub fn run(target: &Target) -> bool {
        match *target {
            Target::Command { window, id } => {
                crate::switch_windows::focus_window(window) && post_command(window, id)
            }
            Target::Automation { window, index } => {
                let Some(automation) = automation() else {
                    return false;
                };
                let items = menu_bar_items(&automation, window);
                let Some(element) = items.get(index) else {
                    return false;
                };
                if !crate::switch_windows::focus_window(window) {
                    return false;
                }
                unsafe {
                    if let Ok(pattern) = element
                        .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                            UIA_ExpandCollapsePatternId,
                        )
                        && pattern.Expand().is_ok()
                    {
                        return true;
                    }
                    element
                        .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                        .and_then(|pattern| pattern.Invoke())
                        .is_ok()
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use windows::{
            Win32::UI::WindowsAndMessaging::{
                AppendMenuW, CreateMenu, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG,
                PM_REMOVE, PeekMessageW, RegisterClassW, SetMenu, WINDOW_EX_STYLE, WNDCLASSW,
                WS_OVERLAPPEDWINDOW,
            },
            core::w,
        };

        use super::*;

        /// A hidden window of this test's own, with a menu bar.
        #[test]
        fn test_reads_a_classic_menu_bar() {
            unsafe {
                let window = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("Menu test"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    100,
                    100,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let bar = CreateMenu().unwrap();
                let file = CreatePopupMenu().unwrap();
                let export = CreatePopupMenu().unwrap();
                AppendMenuW(file, MF_STRING, 10, w!("&Save	Ctrl+S")).unwrap();
                AppendMenuW(file, MF_SEPARATOR, 0, None).unwrap();
                AppendMenuW(export, MF_STRING | MF_GRAYED, 11, w!("As &PDF")).unwrap();
                AppendMenuW(file, MF_POPUP, export.0 as usize, w!("&Export")).unwrap();
                AppendMenuW(bar, MF_POPUP, file.0 as usize, w!("&File")).unwrap();
                SetMenu(window, bar).unwrap();

                let (title, items) = read(window.0 as isize);
                let _ = DestroyWindow(window);
                assert_eq!(title, "Menu test");
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].path, ["File"]);
                assert_eq!(items[0].title, "Save");
                assert_eq!(items[0].shortcut.as_deref(), Some("Ctrl+S"));
                assert!(items[0].enabled);
                assert_eq!(items[1].path, ["File", "Export"]);
                assert_eq!(items[1].title, "As PDF");
                assert!(!items[1].enabled);
                assert_eq!(
                    items[1].target,
                    Target::Command {
                        window: window.0 as isize,
                        id: 11
                    }
                );
            }
        }

        /// The command a window of this test's own receives.
        static RECEIVED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

        unsafe extern "system" fn receive(
            window: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> windows::Win32::Foundation::LRESULT {
            if message == WM_COMMAND {
                RECEIVED.store(wparam.0 as u32, std::sync::atomic::Ordering::Relaxed);
            }
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }

        /// A hidden window of this test's own gets the command, as the menu
        /// would send it.
        #[test]
        fn test_posts_the_menu_command() {
            unsafe {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(receive),
                    lpszClassName: w!("LauncherMenuCommandTest"),
                    ..Default::default()
                };
                RegisterClassW(&class);
                let window = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("LauncherMenuCommandTest"),
                    w!("Menu command test"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    100,
                    100,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                assert!(post_command(window.0 as isize, 42));
                let mut message = MSG::default();
                while PeekMessageW(&mut message, window, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&message);
                }
                let _ = DestroyWindow(window);
                assert_eq!(RECEIVED.load(std::sync::atomic::Ordering::Relaxed), 42);
            }
        }

        /// Reads the menus of every open window; nothing is run.
        /// Run by hand: `cargo test -p launcher read_menus -- --ignored`.
        #[test]
        #[ignore]
        fn test_read_menus_of_open_windows() {
            for window in crate::switch_windows::open_windows() {
                let started = std::time::Instant::now();
                let (title, items) = super::read(window.handle);
                println!(
                    "{title:?}: {} items in {:?}",
                    items.len(),
                    started.elapsed()
                );
                for item in items.iter().take(6) {
                    println!("  {:?} {:?} {:?}", item.path, item.title, item.shortcut);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cleans_access_keys_and_shortcuts() {
        assert_eq!(
            clean_label("Save &As…\tCtrl+Shift+S"),
            ("Save As…".to_owned(), Some("Ctrl+Shift+S".to_owned()))
        );
        assert_eq!(
            clean_label("Tom && Jerry"),
            ("Tom & Jerry".to_owned(), None)
        );
        assert_eq!(clean_label("&File\t"), ("File".to_owned(), None));
    }
}
