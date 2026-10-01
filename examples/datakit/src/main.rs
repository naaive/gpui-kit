//! DataKit, a database IDE built on GPUI Kit.
//!
//! See `docs/DATAKIT-DESIGN.md` for the architecture. In short: the crates
//! under `crates/` know databases and SQL and nothing about windows; this
//! crate composes them into features (`datasource`, `explorer`, `console`,
//! `results`, `history`) that the `workspace` arranges in one window.

mod assets;
mod compare;
mod console;
mod datasource;
mod designer;
mod diagram;
mod dump;
mod explorer;
mod files;
mod format;
mod history;
mod import;
mod navigation;
mod objects;
mod prompt;
mod results;
mod search;
mod services;
mod services_panel;
mod settings;
mod table_editor;
mod workspace;

use crate::{
    assets::AppAssets, datasource::DataSources, history::HistoryLog, navigation::Navigation,
    services::Services, settings::Settings,
};

rust_i18n::i18n!("locales", fallback = "en");

fn main() {
    gpui_kit::application().with_assets(AppAssets).run(|cx| {
        gpui_kit::init(cx);
        rust_i18n::set_locale(system_locale());

        if let Err(error) = Services::init(cx) {
            eprintln!("datakit: cannot start the IO runtime: {error:#}");
            cx.quit();
            return;
        }
        // Menus are named in the chosen language, so it is set first.
        Settings::init(system_locale(), cx);
        Navigation::init(cx);
        DataSources::init(cx);
        HistoryLog::init(cx);
        results::init(cx);
        console::Sessions::init(cx);
        console::init(cx);
        explorer::init(cx);
        files::init(cx);
        table_editor::init(cx);
        workspace::init(cx);

        if let Err(error) = workspace::open_main_window(cx) {
            eprintln!("datakit: cannot open a window: {error:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}

/// The interface language: Chinese in its Simplified or Traditional form
/// when the system asks for it, English otherwise.
fn system_locale() -> &'static str {
    let locale = sys_locale::get_locale().unwrap_or_default();
    let locale = locale.to_ascii_lowercase();
    if !locale.starts_with("zh") {
        "en"
    } else if ["hant", "-hk", "-tw", "-mo"]
        .iter()
        .any(|tag| locale.contains(tag))
    {
        "zh-HK"
    } else {
        "zh-CN"
    }
}
