//! Snip: a screenshot tool in the manner of Snipaste, built on GPUI Kit.
//!
//! Press the capture shortcut (F1) to freeze every display, select a window
//! or drag an area, annotate it, then copy, save or pin it to the screen.
//! See `docs/SNIP-DESIGN.md` for the architecture. In one sentence: native
//! capture produces frames in physical desktop pixels, a pure state machine
//! decides what every input means, and one vector scene is drawn by GPUI
//! for the preview and by tiny-skia for the exported image.

mod app;
mod capture;
mod geometry;
mod output;
mod pin;
#[cfg(feature = "preview")]
mod preview;
mod raster;
mod scene;
mod session;
mod settings_window;
mod shell;

use std::process::ExitCode;

use gpui_kit::QuitMode;

use crate::shell::{
    cli::{self, Command},
    ipc::{Claim, Endpoint, Listener},
};

fn main() -> ExitCode {
    #[cfg(feature = "preview")]
    {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        if let [flag, directory] = arguments.as_slice()
            && flag == "--render-preview"
        {
            init_logging();
            preview::run(directory.into());
            return ExitCode::SUCCESS;
        }
    }
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(command) => command,
        Err(error) => {
            eprintln!("snip: {error}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    let listener = match forward_or_listen(command) {
        Ok(Some(listener)) => listener,
        Ok(None) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("snip: {error:#}");
            return ExitCode::FAILURE;
        }
    };

    init_logging();
    disable_direct_composition();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        // Snip lives in the tray with no window open.
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            gpui_kit::init(cx);
            app::init(cx);
            app::start(app::Startup::new(Some(listener)), cx);
            if let Some(message) = command.message() {
                app::handle(message, cx);
            }
        });
    ExitCode::SUCCESS
}

/// Hands `command` to the running instance, or becomes the instance.
/// Returns the listener to serve when this process is the instance.
fn forward_or_listen(command: Command) -> anyhow::Result<Option<Listener>> {
    let endpoint = Endpoint::for_current_user()?;
    let message = command.message().expect("only help has no message");
    if endpoint.send(&message)? {
        return Ok(None);
    }
    if command == Command::Quit {
        // Nothing is running, so there is nothing to quit.
        return Ok(None);
    }
    match endpoint.claim()? {
        Claim::Listening(listener) => Ok(Some(listener)),
        // Another instance started in between; it handles the request.
        Claim::Running => endpoint.send(&message).map(|_| None),
    }
}

/// Has GPUI draw Snip's windows opaque on Windows.
///
/// GPUI hands every window to DirectComposition with premultiplied alpha,
/// so the compositor blends each frame over what is behind it. Over a
/// display-sized overlay on a 4K HDR desktop, that blending alone keeps an
/// integrated GPU near saturated and dragging feels behind the pointer; an
/// opaque window takes the compositor's cheapest path. Translucent pins
/// fade their whole window instead (`shell::platform::set_window_opacity`).
fn disable_direct_composition() {
    #[cfg(target_os = "windows")]
    if std::env::var_os("GPUI_DISABLE_DIRECT_COMPOSITION").is_none() {
        // SAFETY: no other thread is running yet.
        unsafe { std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1") };
    }
}

/// Logs to stderr at the level `RUST_LOG` names, `info` by default.
fn init_logging() {
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(filter)
        .try_init()
        .ok();
}
