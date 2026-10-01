//! A keyboard-first launcher whose commands come from JavaScript extensions.
//!
//! See `docs/LAUNCHER-DESIGN.md` for the architecture. In one sentence: every
//! page is a `PageModel`, produced in Rust by built-in pages or in JavaScript
//! by extensions, and drawn by the one renderer in `ui`.

mod bookmarks;
mod calendar;
mod clipboard;
mod colors;
mod customizations;
mod emoji;
mod extensions;
mod file_search;
mod focus;
mod format;
mod hyper_key;
mod model;
mod notes;
mod ocr;
mod pages;
mod placeholders;
mod processes;
mod quicklinks;
mod screenshots;
mod script_commands;
mod search;
mod selection;
mod session;
mod shell;
mod snippets;
mod sources;
mod switch_windows;
mod themes;
mod ui;
mod window_layout;

use std::{path::PathBuf, process::ExitCode, rc::Rc};

use gpui_kit::QuitMode;

use crate::{
    extensions::ExtensionHost,
    shell::{
        cli::{self, Command},
        ipc::{Claim, Endpoint, Listener},
        launcher::{self, Startup},
    },
};

fn main() -> ExitCode {
    let current_directory = std::env::current_dir().unwrap_or_default();
    let command = match cli::parse(std::env::args().skip(1), &current_directory) {
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(command) => command,
        Err(error) => {
            eprintln!("launcher: {error}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };

    // Declarations are files in the extension directory; writing them needs
    // no running launcher.
    match &command {
        Command::Types(directory) => {
            return match extensions::write_declarations(directory) {
                Ok(written) => {
                    for path in written {
                        println!("{}", path.display());
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("launcher: {error:#}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Dev(directory) => {
            if let Err(error) = extensions::write_declarations(directory) {
                eprintln!("launcher: cannot write declarations: {error:#}");
            }
        }
        _ => {}
    }

    let listener = match forward_or_listen(&command) {
        Ok(Some(listener)) => listener,
        Ok(None) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("launcher: {error:#}");
            return ExitCode::FAILURE;
        }
    };

    let application = gpui_kit::application()
        // Extensions may name any Lucide icon, so the whole catalog is embedded.
        .with_assets(gpui_kit::assets::AllAssets)
        // The launcher lives on with its window hidden or closed.
        .with_quit_mode(QuitMode::Explicit);
    let (open_urls, opened_urls) = smol::channel::unbounded();
    application.on_open_urls(move |urls| {
        for url in urls {
            open_urls.try_send(url).ok();
        }
    });
    application.run(move |cx| {
        gpui_kit::init(cx);
        gpui_shell::init(cx);
        ui::init(cx);

        let extensions = match ExtensionHost::new(cx) {
            Ok(host) => Rc::new(host),
            Err(error) => {
                eprintln!("cannot start the extension runtime: {error:#}");
                cx.quit();
                return;
            }
        };
        let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions");
        let startup = Startup::new(extensions, bundled)
            .with_listener(listener)
            .with_open_urls(opened_urls);
        let startup = match &command {
            Command::Dev(directory) => startup.with_development_directory(directory.clone()),
            _ => startup,
        };
        launcher::start(startup, cx);
        if let Command::Open(link) = command {
            launcher::handle(shell::ipc::Message::Open(link), cx);
        }
    });
    ExitCode::SUCCESS
}

/// Hands the command to the running instance, or makes this process the
/// instance. Returns the listener to serve, or `None` when there is nothing
/// left for this process to do.
fn forward_or_listen(command: &Command) -> anyhow::Result<Option<Listener>> {
    let endpoint = Endpoint::for_current_user()?;
    let message = command.message().expect("only help has no message");
    if endpoint.send(&message)? {
        return Ok(None);
    }
    if *command == Command::Hide {
        // Nothing is running, so nothing is showing.
        return Ok(None);
    }
    match endpoint.claim()? {
        Claim::Listening(listener) => Ok(Some(listener)),
        // Another launcher started in between; it handles the request.
        Claim::Running => endpoint.send(&message).map(|_| None),
    }
}
