//! The settings window, as Raycast has one: General, Extensions, Advanced
//! and About, opened from Launcher Settings, the tray or a deep link.
//!
//! Every field applies as soon as it changes; there is no Save button.

mod built_in;
mod extensions;
mod hotkey_recorder;
mod inventory;

use std::rc::Rc;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Axis, Bounds, Context, Entity, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
    component::{
        ActiveTheme as _, IconName, Sizable as _, ThemeRegistry,
        button::{Button, ButtonVariants as _},
        h_flex,
        setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings as SettingsView},
        v_flex,
    },
    div, px, size,
};

use self::{
    extensions::ExtensionsPane,
    hotkey_recorder::{HotkeyRecorder, SaveHotkey},
};
use crate::shell::{
    launcher,
    settings::{
        Appearance, EscapeKey, NavigationKeys, PopToRoot, SearchSensitivity, Settings, ShowOn,
        TextSize, WindowMode,
    },
};

#[derive(Default)]
struct OpenSettings(Option<(AnyWindowHandle, Entity<SettingsWindow>)>);

impl Global for OpenSettings {}

/// Shows the settings window, opening it if needed.
pub fn open(cx: &mut App) {
    open_window(None, cx);
}

/// Shows the Extensions settings at the extension or command `id`.
pub fn open_extension(id: String, cx: &mut App) {
    open_window(Some(id), cx);
}

fn open_window(reveal: Option<String>, cx: &mut App) {
    launcher::hide(cx);
    if let Some((handle, view)) = cx
        .try_global::<OpenSettings>()
        .and_then(|open| open.0.clone())
        && cx.windows().contains(&handle)
    {
        let shown = handle
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| view.show(reveal.clone(), window, cx));
                window.activate_window();
            })
            .is_ok();
        if shown {
            cx.activate(true);
            return;
        }
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1060.), px(700.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Launcher Settings".into()),
            ..Default::default()
        }),
        window_min_size: Some(size(px(860.), px(540.))),
        app_id: Some("gpui-kit-launcher".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let mut view = SettingsWindow::new(window, cx);
            view.show(reveal, window, cx);
            view
        })
    }) {
        Ok((handle, view)) => {
            cx.activate(true);
            cx.set_global(OpenSettings(Some((handle, view))));
        }
        Err(error) => tracing::error!("cannot open the settings window: {error:#}"),
    }
}

/// The pages, in sidebar order, by the names links use.
const PAGES: [&str; 4] = ["general", "extensions", "advanced", "about"];
const EXTENSIONS_PAGE: usize = 1;

struct SettingsWindow {
    extensions: Entity<ExtensionsPane>,
    summon: Entity<HotkeyRecorder>,
    /// A page to select on the next render, such as after a deep link.
    select_page: Option<usize>,
    /// Changes whenever a page is selected from outside, so the settings
    /// view starts over on it.
    generation: usize,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let extensions = cx.new(|cx| ExtensionsPane::new(window, cx));
        let save: SaveHotkey = Rc::new(|shortcut, _, cx| {
            let Some(shortcut) = shortcut else {
                return Err("The launcher needs a hotkey to open it.".into());
            };
            crate::shell::hotkey::parse_shortcut(&shortcut).map_err(|error| error.to_string())?;
            change(cx, |settings| settings.with_summon_shortcut(shortcut));
            Ok(())
        });
        let current = launcher::settings(cx).summon_shortcut().to_owned();
        let summon = cx.new(|cx| HotkeyRecorder::new(Some(current), save, cx));
        Self {
            extensions,
            summon,
            select_page: None,
            generation: 0,
        }
    }

    /// Shows the page named `reveal` (`advanced`), or the extension or
    /// command it names on the Extensions page.
    fn show(&mut self, reveal: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let page = reveal.as_deref().and_then(|name| {
            PAGES
                .iter()
                .position(|page| page.eq_ignore_ascii_case(name))
        });
        self.extensions.update(cx, |pane, cx| {
            pane.reload(cx);
            if let (Some(id), None) = (&reveal, page) {
                pane.reveal(id, window, cx);
            }
        });
        if reveal.is_some() {
            self.select_page = Some(page.unwrap_or(EXTENSIONS_PAGE));
            self.generation += 1;
        }
        cx.notify();
    }

    fn pages(&self, window: &Window, cx: &App) -> Vec<SettingPage> {
        vec![
            self.general_page(cx),
            self.extensions_page(window),
            advanced_page(),
            about_page(cx),
        ]
    }

    fn general_page(&self, cx: &App) -> SettingPage {
        let summon = self.summon.clone();
        let status = launcher::hotkey_status(cx)
            .filter(|status| *status != crate::shell::hotkey::HotkeyStatus::Registered)
            .map(|status| status.to_string());
        SettingPage::new("General")
            .icon(IconName::Settings2)
            .groups(vec![
                SettingGroup::new().title("Startup").items(vec![
                    SettingItem::new(
                        "Launcher hotkey",
                        SettingField::render(move |_, _, _| summon.clone()),
                    )
                    .description(match status {
                        Some(status) => status,
                        None => "Opens the launcher from any application. Click the field and press the keys.".into(),
                    }),
                    SettingItem::new(
                        "Launch at login",
                        SettingField::switch(
                            |cx: &App| launcher::settings(cx).launch_at_login(),
                            |value: bool, cx: &mut App| {
                                change(cx, |settings| settings.with_launch_at_login(value))
                            },
                        )
                        .default_value(false),
                    ),
                    SettingItem::new(
                        "Show icon in tray",
                        SettingField::switch(
                            |cx: &App| launcher::settings(cx).shows_tray_icon(),
                            |value: bool, cx: &mut App| {
                                change(cx, |settings| settings.with_show_tray_icon(value))
                            },
                        )
                        .default_value(true),
                    )
                    .description("Opens the launcher and its settings from the notification area."),
                ]),
                SettingGroup::new().title("Appearance").items(vec![
                    SettingItem::new(
                        "Appearance",
                        choice_field(
                            Appearance::ALL.iter().map(|choice| choice.title()).collect(),
                            |settings| settings.appearance().title(),
                            |settings, title| {
                                Appearance::ALL
                                    .iter()
                                    .find(|choice| choice.title() == title)
                                    .map(|choice| settings.with_appearance(*choice))
                            },
                        ),
                    ),
                    SettingItem::new("Light theme", theme_field(false, cx)),
                    SettingItem::new("Dark theme", theme_field(true, cx)),
                    SettingItem::new(
                        "Text size",
                        choice_field(
                            TextSize::ALL.iter().map(|choice| choice.title()).collect(),
                            |settings| settings.text_size().title(),
                            |settings, title| {
                                TextSize::from_title(title).map(|size| settings.with_text_size(size))
                            },
                        ),
                    ),
                    SettingItem::new(
                        "Window mode",
                        choice_field(
                            WindowMode::ALL.iter().map(|choice| choice.title()).collect(),
                            |settings| settings.window_mode().title(),
                            |settings, title| {
                                WindowMode::ALL
                                    .iter()
                                    .find(|choice| choice.title() == title)
                                    .map(|mode| settings.with_window_mode(*mode))
                            },
                        ),
                    )
                    .description("Compact shows only the search field until you type."),
                    SettingItem::new(
                        "Favorites in compact mode",
                        SettingField::switch(
                            |cx: &App| launcher::settings(cx).shows_favorites_in_compact(),
                            |value: bool, cx: &mut App| {
                                change(cx, |settings| settings.with_favorites_in_compact(value))
                            },
                        )
                        .default_value(true),
                    )
                    .description("Lists your favorites under the search field before you type."),
                ]),
            ])
    }

    fn extensions_page(&self, window: &Window) -> SettingPage {
        let pane = self.extensions.clone();
        // The table fills the window below the page's title.
        let height = (window.viewport_size().height - px(124.)).max(px(320.));
        SettingPage::new("Extensions")
            .icon(IconName::LayoutDashboard)
            .group(
                SettingGroup::new().item(
                    SettingItem::render(move |_, _, _| {
                        div().w_full().h(height).child(pane.clone())
                    })
                    .keywords([
                        "commands",
                        "alias",
                        "hotkey",
                        "shortcut",
                        "disable",
                        "enable",
                        "preferences",
                    ]),
                ),
            )
    }
}

fn advanced_page() -> SettingPage {
    let mut keyboard = vec![
        SettingItem::new(
            "Navigation keys",
            choice_field(
                NavigationKeys::ALL
                    .iter()
                    .map(|choice| choice.title())
                    .collect(),
                |settings| settings.navigation_keys().title(),
                |settings, title| {
                    NavigationKeys::from_title(title)
                        .map(|keys| settings.with_navigation_keys(keys))
                },
            ),
        )
        .description("Move the selection with these as well as the arrow keys."),
        SettingItem::new(
            "Escape key",
            choice_field(
                EscapeKey::ALL.iter().map(|choice| choice.title()).collect(),
                |settings| settings.escape_key().title(),
                |settings, title| {
                    EscapeKey::from_title(title).map(|key| settings.with_escape_key(key))
                },
            ),
        ),
    ];
    if crate::hyper_key::is_supported() {
        keyboard.push(
            SettingItem::new(
                "Hyper Key",
                choice_field(
                    crate::hyper_key::HyperKey::ALL
                        .iter()
                        .map(|choice| choice.title())
                        .collect(),
                    |settings| settings.hyper_key().title(),
                    |settings, title| {
                        crate::hyper_key::HyperKey::ALL
                            .iter()
                            .find(|choice| choice.title() == title)
                            .map(|key| settings.with_hyper_key(*key))
                    },
                ),
            )
            .description("Turns Caps Lock into Ctrl+Alt+Shift+Win, for hotkeys of your own."),
        );
    }
    SettingPage::new("Advanced")
        .icon(gpui_kit::component::Icon::empty().path("icons/sliders-horizontal.svg"))
        .groups(vec![
            SettingGroup::new().title("Window").items(vec![
                SettingItem::new(
                    "Show launcher on",
                    choice_field(
                        ShowOn::ALL.iter().map(|choice| choice.title()).collect(),
                        |settings| settings.show_on().title(),
                        |settings, title| ShowOn::from_title(title).map(|show_on| settings.with_show_on(show_on)),
                    ),
                ),
                SettingItem::new(
                    "Return to root search",
                    choice_field(
                        PopToRoot::ALL.iter().map(|choice| choice.title()).collect(),
                        |settings| settings.pop_to_root().title(),
                        |settings, title| {
                            PopToRoot::ALL
                                .iter()
                                .find(|choice| choice.title() == title)
                                .map(|when| settings.with_pop_to_root(*when))
                        },
                    ),
                )
                .description("When the launcher, opened again, starts over instead of showing the last command."),
            ]),
            SettingGroup::new().title("Keyboard").items(keyboard),
            SettingGroup::new().title("Search").item(
                SettingItem::new(
                    "Search sensitivity",
                    choice_field(
                        SearchSensitivity::ALL.iter().map(|choice| choice.title()).collect(),
                        |settings| settings.search_sensitivity().title(),
                        |settings, title| {
                            SearchSensitivity::from_title(title)
                                .map(|sensitivity| settings.with_search_sensitivity(sensitivity))
                        },
                    ),
                )
                .description("High also finds titles with the letters spread out; Low only titles containing what you type."),
            ),
            SettingGroup::new().title("Extensions").items(vec![
                SettingItem::new(
                    "Extensions folder",
                    SettingField::input(
                        |cx: &App| {
                            launcher::settings(cx)
                                .extension_directory()
                                .map(|path| path.display().to_string())
                                .unwrap_or_default()
                                .into()
                        },
                        |value: SharedString, cx: &mut App| {
                            let value = value.trim();
                            let directory = crate::shell::settings::expand_home(value);
                            if value.is_empty() || directory.is_dir() {
                                change(cx, |settings| {
                                    settings.with_extension_directory((!value.is_empty()).then_some(directory))
                                });
                            }
                        },
                    ),
                )
                .layout(Axis::Vertical)
                .description("Extensions here load before the installed ones; for developing your own."),
                SettingItem::new(
                    "Extension Store",
                    SettingField::input(
                        |cx: &App| launcher::settings(cx).store_source().unwrap_or_default().to_owned().into(),
                        |value: SharedString, cx: &mut App| {
                            let value = value.trim().to_owned();
                            if value.is_empty()
                                || crate::extensions::store::StoreSource::parse(&value).is_ok()
                            {
                                change(cx, |settings| {
                                    settings.with_store_source((!value.is_empty()).then_some(value))
                                });
                            }
                        },
                    ),
                )
                .layout(Axis::Vertical)
                .description("owner/repo@ref/folder on GitHub, or a folder. Leave empty for the default store."),
            ]),
            SettingGroup::new().title("Network").item(
                SettingItem::new(
                    "Proxy",
                    SettingField::input(
                        |cx: &App| launcher::settings(cx).proxy().unwrap_or_default().to_owned().into(),
                        |value: SharedString, cx: &mut App| {
                            let value = value.trim().to_owned();
                            if value.is_empty() || is_proxy_url(&value) {
                                change(cx, |settings| settings.with_proxy((!value.is_empty()).then_some(value)));
                            }
                        },
                    ),
                )
                .layout(Axis::Vertical)
                .description("For the launcher’s and extensions’ requests, such as http://127.0.0.1:7890. Leave empty to connect directly."),
            ),
            SettingGroup::new().title("Data").item(SettingItem::render(|_, _, _| {
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("export-data")
                            .outline()
                            .small()
                            .label("Export Settings & Data…")
                            .on_click(|_, _, cx| launcher::open_item("system/export-data".into(), cx)),
                    )
                    .child(
                        Button::new("import-data")
                            .outline()
                            .small()
                            .label("Import…")
                            .on_click(|_, _, cx| launcher::open_item("system/import-data".into(), cx)),
                    )
            })),
        ])
}

fn about_page(cx: &App) -> SettingPage {
    let data = crate::shell::data_directory();
    let muted = cx.theme().muted_foreground;
    SettingPage::new("About")
        .icon(IconName::Info)
        .group(
            SettingGroup::new().item(SettingItem::render(move |_, _, _| {
                let data = data.clone();
                v_flex()
                    .gap_3()
                    .child(div().text_lg().child("GPUI Kit Launcher"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(div().text_sm().text_color(muted).child(
                        "A launcher for apps, commands and extensions, built with GPUI Kit.",
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("website")
                                    .outline()
                                    .small()
                                    .label("gpui-kit.com")
                                    .on_click(|_, _, cx| cx.open_url("https://gpui-kit.com")),
                            )
                            .child(
                                Button::new("data-folder")
                                    .ghost()
                                    .small()
                                    .label("Show Data Folder")
                                    .on_click(move |_, _, cx| {
                                        if let Some(data) = &data {
                                            cx.open_url(&format!("file:///{}", data.display()));
                                        }
                                    }),
                            ),
                    )
            })),
        )
}

/// Whether `value` looks like a proxy address the HTTP client accepts.
fn is_proxy_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") && url.host().is_some()
    })
}

/// Applies `change` to the settings and saves them.
fn change(cx: &mut App, change: impl FnOnce(Settings) -> Settings) {
    let settings = change(launcher::settings(cx));
    if let Err(error) = launcher::update_settings(settings, None, cx) {
        tracing::warn!("cannot save the settings: {error:#}");
    }
}

/// A dropdown of fixed choices, by their titles.
fn choice_field(
    titles: Vec<&'static str>,
    value: fn(&Settings) -> &'static str,
    set: fn(Settings, &str) -> Option<Settings>,
) -> SettingField<SharedString> {
    SettingField::dropdown(
        titles
            .into_iter()
            .map(|title| (title.into(), title.into()))
            .collect(),
        move |cx: &App| value(&launcher::settings(cx)).into(),
        move |title: SharedString, cx: &mut App| {
            if let Some(settings) = set(launcher::settings(cx), &title) {
                if let Err(error) = launcher::update_settings(settings, None, cx) {
                    tracing::warn!("cannot save the settings: {error:#}");
                }
            }
        },
    )
}

/// The theme for light or dark appearance, from every registered theme of
/// that kind.
fn theme_field(dark: bool, cx: &App) -> SettingField<SharedString> {
    let names: Vec<(SharedString, SharedString)> = ThemeRegistry::global(cx)
        .sorted_themes()
        .into_iter()
        .filter(|theme| theme.mode.is_dark() == dark)
        .map(|theme| (theme.name.clone(), theme.name.clone()))
        .collect();
    SettingField::scrollable_dropdown(
        names,
        move |cx: &App| crate::themes::current_name(dark, cx),
        move |name: SharedString, cx: &mut App| {
            change(cx, |settings| {
                settings.with_theme(dark, Some(name.to_string()))
            });
        },
    )
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = SettingsView::new(SharedString::from(format!(
            "launcher-settings-{}",
            self.generation
        )))
        .sidebar_width(px(200.))
        .pages(self.pages(window, cx));
        match self.select_page.take() {
            Some(page) => view.default_selected_index(gpui_kit::component::setting::SelectIndex {
                page_ix: page,
                group_ix: None,
            }),
            None => view,
        }
    }
}
