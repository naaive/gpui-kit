//! Search Browser Tabs: the tabs open in every browser window, to switch
//! to. Windows only; elsewhere the command is not offered.
//!
//! Tabs are read through UI Automation, the accessibility API: the tab
//! strip is the first tab list of a browser window. Only the selected tab of
//! each window shows its address, since the others have no address bar.

use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};

use crate::{
    bookmarks::host,
    model::{
        Accessory, Action, ActionPanel, Effect, Image, Item, ItemId, ListModel, PageModel,
        RunHandler, Section,
    },
    pages::{self, Page, PageHandle},
    sources::applications::file_icon,
};

/// One tab of a browser window.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    /// The window's raw handle.
    pub window: isize,
    /// Its place in the window's tab strip.
    pub index: usize,
    pub title: String,
    /// The browser's name, as its executable is called.
    pub browser: String,
    pub exe: Option<PathBuf>,
    pub selected: bool,
    /// The address, for the selected tab of each window.
    pub url: Option<String>,
}

/// A browser's name as people write it, from its executable's.
fn browser_name(executable: &str) -> String {
    match executable.to_lowercase().as_str() {
        "chrome" => "Chrome".into(),
        "msedge" => "Edge".into(),
        "firefox" => "Firefox".into(),
        "brave" => "Brave".into(),
        "opera" => "Opera".into(),
        "vivaldi" => "Vivaldi".into(),
        "arc" => "Arc".into(),
        "360chrome" => "360 Browser".into(),
        _ => executable.to_owned(),
    }
}

pub fn is_supported() -> bool {
    cfg!(target_os = "windows")
}

/// Every tab of every browser window, front window first. Blocking.
fn open_tabs() -> Vec<Tab> {
    #[cfg(target_os = "windows")]
    {
        windows::open_tabs()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// Selects `tab` in its window and brings the window to the front.
fn switch_to(tab: &Tab) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::select(tab) && crate::switch_windows::focus_window(tab.window)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = tab;
        false
    }
}

pub fn search_tabs_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|cx| {
        let task = cx.spawn(async move |this, cx| {
            let tabs = cx.background_spawn(async { open_tabs() }).await;
            let exes: Vec<PathBuf> = tabs.iter().filter_map(|tab| tab.exe.clone()).collect();
            this.update(cx, |page: &mut TabsPage, cx| {
                page.tabs = Some(tabs);
                cx.notify();
            })
            .ok();
            let icons = cx
                .background_spawn(async move {
                    let mut icons = HashMap::new();
                    for exe in exes {
                        if !icons.contains_key(&exe) {
                            let icon = file_icon(&exe);
                            icons.insert(exe, icon);
                        }
                    }
                    icons
                })
                .await;
            this.update(cx, |page, cx| {
                page.icons = icons;
                cx.notify();
            })
            .ok();
        });
        TabsPage {
            tabs: None,
            icons: HashMap::new(),
            _task: task,
        }
    })))
}

struct TabsPage {
    tabs: Option<Vec<Tab>>,
    icons: HashMap<PathBuf, Option<PathBuf>>,
    _task: Task<()>,
}

impl TabsPage {
    fn item(&self, tab: &Tab) -> Item {
        let image = tab
            .exe
            .as_ref()
            .and_then(|exe| self.icons.get(exe).cloned().flatten())
            .map(Image::File)
            .unwrap_or(Image::Icon("panels-top-left".into()));
        let switch = tab.clone();
        let mut actions = ActionPanel::new().with_action(
            Action::new(
                "Switch to Tab",
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    let tab = switch.clone();
                    crate::shell::launcher::hide(cx);
                    cx.background_spawn(async move { switch_to(&tab) }).detach();
                })),
            )
            .with_image(Image::Icon("panels-top-left".into())),
        );
        if let Some(url) = &tab.url {
            actions = actions
                .with_action(
                    Action::new("Copy URL", Effect::Copy(url.clone().into()))
                        .with_image(Image::Icon("copy".into()))
                        .with_shortcut("secondary-shift-c"),
                )
                .with_action(
                    Action::new(
                        "Copy as Markdown Link",
                        Effect::Copy(format!("[{}]({url})", tab.title).into()),
                    )
                    .with_image(Image::Icon("link".into())),
                );
        }
        actions = actions.with_action(
            Action::new("Copy Title", Effect::Copy(tab.title.clone().into()))
                .with_image(Image::Icon("type".into())),
        );
        let mut item = Item::new(
            ItemId::new(format!("tab/{}/{}", tab.window, tab.index)),
            tab.title.clone(),
        )
        .with_image(image)
        .with_keyword(tab.browser.clone())
        .with_actions(actions);
        if let Some(url) = &tab.url {
            item = item.with_subtitle(host(url)).with_keyword(url.clone());
        }
        if tab.selected {
            item = item.with_accessory(Accessory::text("Current"));
        }
        item.with_accessory(Accessory::text(browser_name(&tab.browser)))
    }
}

impl Page for TabsPage {
    fn title(&self) -> SharedString {
        "Browser Tabs".into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let tabs = self.tabs.as_deref().unwrap_or_default();
        // A section per window, front to back.
        let mut sections: Vec<(isize, Vec<Item>)> = Vec::new();
        for tab in tabs {
            let item = self.item(tab);
            match sections
                .iter_mut()
                .find(|(window, _)| *window == tab.window)
            {
                Some((_, items)) => items.push(item),
                None => sections.push((tab.window, vec![item])),
            }
        }
        let count = sections.len();
        sections.into_iter().enumerate().fold(
            ListModel::new()
                .with_placeholder("Search open tabs…")
                .with_loading(self.tabs.is_none())
                .with_empty_title(match self.tabs {
                    None => "Reading tabs…",
                    Some(_) => "No browser tabs open",
                })
                .with_empty_description(
                    "Tabs of Chrome, Edge, Firefox, Brave, Opera and Vivaldi windows are listed.",
                ),
            |list, (ix, (_, items))| {
                let title = match count {
                    1 => "Tabs".to_owned(),
                    _ => format!("Window {}", ix + 1),
                };
                list.with_section(Section::new().with_title(title).with_items(items))
            },
        )
        .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(target_os = "windows")]
mod windows {
    use windows::{
        Win32::{
            Foundation::HWND,
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            },
            UI::Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationElement,
                IUIAutomationLegacyIAccessiblePattern, IUIAutomationSelectionItemPattern,
                TreeScope_Descendants, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
                UIA_LegacyIAccessiblePatternId, UIA_SelectionItemPatternId,
                UIA_TabItemControlTypeId,
            },
        },
        core::VARIANT,
    };

    use super::Tab;
    use crate::focus::browser;

    fn automation() -> Option<IUIAutomation> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()
        }
    }

    /// The tabs of the window `handle`, in strip order: its tab items,
    /// less those of the page it shows, which are inside a document.
    fn tab_elements(automation: &IUIAutomation, handle: isize) -> Vec<IUIAutomationElement> {
        unsafe {
            let Ok(root) = automation.ElementFromHandle(HWND(handle as *mut std::ffi::c_void))
            else {
                return Vec::new();
            };
            let (Ok(condition), Ok(walker)) = (
                automation.CreatePropertyCondition(
                    UIA_ControlTypePropertyId,
                    &VARIANT::from(UIA_TabItemControlTypeId.0),
                ),
                automation.ControlViewWalker(),
            ) else {
                return Vec::new();
            };
            let Ok(items) = root.FindAll(TreeScope_Descendants, &condition) else {
                return Vec::new();
            };
            let in_document = |element: &IUIAutomationElement| {
                let mut current = element.clone();
                // A tab strip is a few levels below the window.
                for _ in 0..32 {
                    let Ok(parent) = walker.GetParentElement(&current) else {
                        return false;
                    };
                    if parent.CurrentControlType() == Ok(UIA_DocumentControlTypeId) {
                        return true;
                    }
                    if automation
                        .CompareElements(&parent, &root)
                        .is_ok_and(|same| same.as_bool())
                    {
                        return false;
                    }
                    current = parent;
                }
                false
            };
            let length = items.Length().unwrap_or(0);
            (0..length)
                .filter_map(|ix| items.GetElement(ix).ok())
                .filter(|element| !in_document(element))
                .collect()
        }
    }

    fn is_selected(element: &IUIAutomationElement) -> bool {
        unsafe {
            element
                .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                    UIA_SelectionItemPatternId,
                )
                .and_then(|pattern| pattern.CurrentIsSelected())
                .is_ok_and(|selected| selected.as_bool())
        }
    }

    /// What Chromium appends to a tab's name for screen readers.
    const ANNOTATIONS: [&str; 5] = [
        " - High memory usage",
        " - Memory usage",
        " - Audio playing",
        " - Audio muted",
        " - Pinned",
    ];

    /// The tab's title, without what the browser appends to it.
    fn name(element: &IUIAutomationElement) -> String {
        let name = unsafe { element.CurrentName() }
            .map(|name| name.to_string())
            .unwrap_or_default();
        let end = ANNOTATIONS
            .iter()
            .filter_map(|annotation| name.find(annotation))
            .min()
            .unwrap_or(name.len());
        name[..end].trim().to_owned()
    }

    pub fn open_tabs() -> Vec<Tab> {
        let Some(automation) = automation() else {
            return Vec::new();
        };
        let mut tabs = Vec::new();
        for window in crate::switch_windows::open_windows() {
            let Some(application) = window.application() else {
                continue;
            };
            if !browser::is_browser(&application) {
                continue;
            }
            let elements = tab_elements(&automation, window.handle);
            let mut found: Vec<Tab> = elements
                .iter()
                .enumerate()
                .map(|(index, element)| Tab {
                    window: window.handle,
                    index,
                    title: name(element),
                    browser: application.clone(),
                    exe: window.exe.clone(),
                    selected: is_selected(element),
                    url: None,
                })
                .filter(|tab| !tab.title.is_empty())
                .collect();
            if let Some(selected) = found.iter_mut().find(|tab| tab.selected) {
                selected.url = browser::address(window.handle);
            }
            tabs.extend(found);
        }
        tabs
    }

    /// Selects `tab` in its window, if it is still where it was.
    pub fn select(tab: &Tab) -> bool {
        let Some(automation) = automation() else {
            return false;
        };
        let elements = tab_elements(&automation, tab.window);
        // The strip may have changed since it was read; find the tab by its
        // title near where it was.
        let Some(element) = elements
            .get(tab.index)
            .filter(|element| name(element) == tab.title)
            .or_else(|| elements.iter().find(|element| name(element) == tab.title))
        else {
            return false;
        };
        unsafe {
            if let Ok(pattern) = element.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                UIA_SelectionItemPatternId,
            ) && pattern.Select().is_ok()
            {
                return true;
            }
            element
                .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                    UIA_LegacyIAccessiblePatternId,
                )
                .and_then(|pattern| pattern.DoDefaultAction())
                .is_ok()
        }
    }

    #[cfg(test)]
    mod tests {
        use windows::{
            Win32::{
                Foundation::{LPARAM, WPARAM},
                UI::{
                    Controls::{
                        ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TCIF_TEXT,
                        TCITEMW, TCM_GETCURSEL, TCM_INSERTITEMW, WC_TABCONTROLW,
                    },
                    WindowsAndMessaging::{
                        CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
                        PeekMessageW, SW_SHOWNOACTIVATE, SendMessageW, ShowWindow,
                        TranslateMessage, WINDOW_EX_STYLE, WS_CHILD, WS_EX_NOACTIVATE,
                        WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
                    },
                },
            },
            core::{PWSTR, w},
        };

        use super::*;

        /// A tab strip in a window of this test's own, off screen and never
        /// activated: the tabs are read and one is selected through UI
        /// Automation, as a browser's are.
        #[test]
        fn test_reads_and_selects_tabs() {
            unsafe {
                let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                    dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: ICC_TAB_CLASSES,
                });
                let frame = CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    w!("Tabs test"),
                    WS_POPUP,
                    -4000,
                    -4000,
                    400,
                    100,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let strip = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    WC_TABCONTROLW,
                    w!(""),
                    WS_CHILD | WS_VISIBLE,
                    0,
                    0,
                    400,
                    40,
                    frame,
                    None,
                    None,
                    None,
                )
                .unwrap();
                for (ix, title) in ["Inbox", "Docs - High memory usage - 1.1 GB", "News"]
                    .iter()
                    .enumerate()
                {
                    let mut text: Vec<u16> = title.encode_utf16().chain([0]).collect();
                    let item = TCITEMW {
                        mask: TCIF_TEXT,
                        pszText: PWSTR(text.as_mut_ptr()),
                        ..Default::default()
                    };
                    SendMessageW(
                        strip,
                        TCM_INSERTITEMW,
                        WPARAM(ix),
                        LPARAM(&item as *const TCITEMW as isize),
                    );
                }
                let _ = ShowWindow(frame, SW_SHOWNOACTIVATE);

                // UI Automation asks the window's thread, so this one keeps
                // its messages flowing while another reads.
                let handle = frame.0 as isize;
                let reader = std::thread::spawn(move || {
                    let automation = automation().unwrap();
                    let names: Vec<String> =
                        tab_elements(&automation, handle).iter().map(name).collect();
                    let selected = select(&Tab {
                        window: handle,
                        index: 1,
                        title: "Docs".into(),
                        browser: "test".into(),
                        exe: None,
                        selected: false,
                        url: None,
                    });
                    (names, selected)
                });
                let mut message = MSG::default();
                while !reader.is_finished() {
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                let (names, selected) = reader.join().unwrap();
                let current = SendMessageW(strip, TCM_GETCURSEL, WPARAM(0), LPARAM(0)).0;
                let _ = DestroyWindow(frame);
                assert_eq!(names, ["Inbox", "Docs", "News"]);
                assert!(selected);
                assert_eq!(current, 1, "the second tab is selected");
            }
        }

        /// Reads the tabs of the open browser windows; nothing is changed.
        /// Run by hand: `cargo test -p launcher open_tabs -- --ignored`.
        #[test]
        #[ignore]
        fn test_lists_open_tabs() {
            let started = std::time::Instant::now();
            let tabs = super::open_tabs();
            println!("{} tabs in {:?}", tabs.len(), started.elapsed());
            for tab in tabs {
                println!("{tab:?}");
            }
        }
    }
}
