//! A keyboard-first launcher whose commands come from JavaScript extensions.
//!
//! See `docs/LAUNCHER-DESIGN.md` for the architecture. In one sentence: every
//! page is a `PageModel`, produced in Rust by built-in pages or in JavaScript
//! by extensions, and drawn by the one renderer in `ui`.

mod extensions;
mod model;
mod pages;
mod search;
mod session;
mod ui;

use std::{path::PathBuf, rc::Rc};

use gpui_kit::{
    AppContext as _, TitlebarOptions, WindowBounds, WindowKind, WindowOptions, px, size,
};

use crate::{
    extensions::{Catalog, ExtensionHost},
    ui::LauncherWindow,
};

/// Extension directories, searched in order; an earlier one wins a duplicate
/// id. `LAUNCHER_EXTENSIONS` adds a directory ahead of the bundled examples,
/// for developing an extension outside this repository.
fn extension_roots() -> Vec<PathBuf> {
    std::env::var_os("LAUNCHER_EXTENSIONS")
        .map(PathBuf::from)
        .into_iter()
        .chain([PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions")])
        .collect()
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            gpui_shell::init(cx);
            ui::init(cx);

            let catalog = Rc::new(Catalog::discover(&extension_roots()));
            let extensions = match ExtensionHost::new(cx) {
                Ok(host) => Rc::new(host),
                Err(error) => {
                    eprintln!("cannot start the extension runtime: {error:#}");
                    cx.quit();
                    return;
                }
            };

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(750.), px(475.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Launcher".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                kind: WindowKind::Normal,
                is_resizable: false,
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| LauncherWindow::new(catalog, extensions, window, cx))
            })
            .expect("failed to open the launcher window");
            cx.activate(true);
        });
}
