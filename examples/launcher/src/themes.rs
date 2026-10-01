//! Themes: the color themes that ship with GPUI Kit, and the user's own
//! from the launcher's themes folder, one chosen for light appearance and
//! one for dark.

use std::{path::PathBuf, rc::Rc};

use anyhow::Result;
use gpui_kit::{
    App, AppContext as _, Context, SharedString, Window,
    component::{ActiveTheme as _, Theme, ThemeConfig, ThemeMode, ThemeRegistry},
};

use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, Effect, Image, Item, ItemId,
        ListModel, PageModel, PushHandler, RunHandler, Section, Tone,
    },
    pages::{self, Page, PageHandle},
    shell::launcher,
};

/// The themes in GPUI Kit's `themes` folder.
const BUNDLED: [&str; 21] = [
    include_str!("../../../themes/adventure.json"),
    include_str!("../../../themes/alduin.json"),
    include_str!("../../../themes/asciinema.json"),
    include_str!("../../../themes/aurora.json"),
    include_str!("../../../themes/ayu.json"),
    include_str!("../../../themes/catppuccin.json"),
    include_str!("../../../themes/everforest.json"),
    include_str!("../../../themes/fahrenheit.json"),
    include_str!("../../../themes/flexoki.json"),
    include_str!("../../../themes/gruvbox.json"),
    include_str!("../../../themes/harper.json"),
    include_str!("../../../themes/hybrid.json"),
    include_str!("../../../themes/jellybeans.json"),
    include_str!("../../../themes/kibble.json"),
    include_str!("../../../themes/macos-classic.json"),
    include_str!("../../../themes/mellifluous.json"),
    include_str!("../../../themes/molokai.json"),
    include_str!("../../../themes/solarized.json"),
    include_str!("../../../themes/spaceduck.json"),
    include_str!("../../../themes/tokyonight.json"),
    include_str!("../../../themes/twilight.json"),
];

/// Where the user's own theme files go, in GPUI Kit's theme format.
pub fn directory() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("themes"))
}

/// Adds the bundled themes and the user's to the registry.
pub fn register(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    for source in BUNDLED {
        if let Err(error) = registry.load_themes_from_str(source) {
            tracing::warn!("cannot read a bundled theme: {error}");
        }
    }
    let Some(directory) = directory() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && let Ok(source) = std::fs::read_to_string(&path)
            && let Err(error) = ThemeRegistry::global_mut(cx).load_themes_from_str(&source)
        {
            tracing::warn!("cannot read the theme {}: {error}", path.display());
        }
    }
}

fn named(name: Option<&str>, cx: &App) -> Option<Rc<ThemeConfig>> {
    ThemeRegistry::global(cx).themes().get(name?).cloned()
}

/// Uses the chosen themes for light and dark appearance; a theme that no
/// longer exists falls back to the default.
pub fn apply(light: Option<&str>, dark: Option<&str>, cx: &mut App) {
    let registry = ThemeRegistry::global(cx);
    let light = named(light, cx).unwrap_or_else(|| registry.default_light_theme().clone());
    let dark =
        named(dark, cx).unwrap_or_else(|| ThemeRegistry::global(cx).default_dark_theme().clone());
    let mode = match cx.theme().is_dark() {
        true => ThemeMode::Dark,
        false => ThemeMode::Light,
    };
    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
    });
    // Loads the chosen theme of the current mode.
    Theme::change(mode, None, cx);
}

pub fn change_theme_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|_| ThemesPage)))
}

struct ThemesPage;

/// Chooses `name` for its appearance, and shows it.
fn choose(config: &Rc<ThemeConfig>, window: &mut Window, cx: &mut App) {
    let dark = config.mode.is_dark();
    let mut settings = launcher::settings(cx);
    settings = settings.with_theme(dark, Some(config.name.to_string()));
    // A fixed appearance of the other kind would hide the choice.
    let appearance = settings.appearance();
    let shows_other = match appearance {
        crate::shell::settings::Appearance::Light => dark,
        crate::shell::settings::Appearance::Dark => !dark,
        crate::shell::settings::Appearance::System => false,
    };
    if shows_other {
        settings = settings.with_appearance_value(match dark {
            true => crate::shell::settings::Appearance::Dark,
            false => crate::shell::settings::Appearance::Light,
        });
    }
    let name = config.name.clone();
    match launcher::update_settings(settings, window, cx) {
        Ok(()) => launcher::perform(
            Effect::ShowToast(crate::model::Toast::new(
                crate::model::ToastStyle::Success,
                match dark {
                    true => format!("Dark theme: {name}"),
                    false => format!("Light theme: {name}"),
                },
            )),
            cx,
        ),
        Err(error) => tracing::warn!("cannot save the theme: {error:#}"),
    }
}

impl Page for ThemesPage {
    fn title(&self) -> SharedString {
        "Themes".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let settings = launcher::settings(cx);
        let registry = ThemeRegistry::global(cx);
        let themes = registry.sorted_themes();
        let current = |config: &ThemeConfig| {
            let chosen = settings.theme(config.mode.is_dark());
            match chosen {
                Some(chosen) => chosen == config.name.as_ref(),
                None => config.is_default,
            }
        };
        let open_folder = Action::new(
            "Open Themes Folder",
            Effect::Run(RunHandler::new(|(), _, cx| {
                if let Some(directory) = directory() {
                    std::fs::create_dir_all(&directory).ok();
                    launcher::perform(Effect::OpenPath(directory), cx);
                }
            })),
        )
        .with_image(Image::Icon("folder-open".into()))
        .with_shortcut("secondary-shift-o");
        let item = |config: &Rc<ThemeConfig>| {
            let chosen = config.clone();
            let dark = config.mode.is_dark();
            let item = Item::new(
                ItemId::new(format!("theme:{}", config.name)),
                config.name.clone(),
            )
            .with_icon(match dark {
                true => "moon",
                false => "sun",
            })
            .with_keyword(match dark {
                true => "dark",
                false => "light",
            })
            .with_actions(
                ActionPanel::new()
                    .with_action(Action::new(
                        match dark {
                            true => "Use as Dark Theme",
                            false => "Use as Light Theme",
                        },
                        Effect::Run(RunHandler::new(move |(), window, cx| {
                            choose(&chosen, window, cx)
                        })),
                    ))
                    .with_section(
                        ActionSection::new().with_entry(ActionEntry::Action(open_folder.clone())),
                    ),
            );
            match current(config) {
                true => item.with_accessory(Accessory::tag("In Use", Tone::Accent)),
                false => item,
            }
        };
        let (dark, light): (Vec<_>, Vec<_>) =
            themes.into_iter().partition(|config| config.mode.is_dark());
        let light = Section::new()
            .with_title("Light")
            .with_items(light.into_iter().map(item));
        let dark = Section::new()
            .with_title("Dark")
            .with_items(dark.into_iter().map(item));
        // The themes of the appearance showing now come first.
        let (first, second) = match cx.theme().is_dark() {
            true => (dark, light),
            false => (light, dark),
        };
        ListModel::new()
            .with_placeholder("Search themes…")
            .with_section(first)
            .with_section(second)
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

/// The root search command.
pub fn commands() -> Vec<Item> {
    vec![
        Item::new(ItemId::new("system/change-theme"), "Change Theme")
            .with_icon("palette")
            .with_accessory(Accessory::text("Command"))
            .with_keyword("theme")
            .with_keyword("appearance")
            .with_keyword("colors")
            .with_action(Action::new(
                "Change Theme",
                Effect::Push(PushHandler::new(change_theme_page)),
            )),
    ]
}
