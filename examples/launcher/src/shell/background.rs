//! Commands the launcher runs on its own: `no-view` commands on their
//! `interval`, and `menu-bar` commands, whose render is a tray icon and menu.
//!
//! An extension's code needs a window to be loaded in, and the launcher's
//! own window is closed whenever it is hidden on Windows and Linux. So
//! background runs get a window of their own: never shown, never focused,
//! one pixel off screen.

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use anyhow::{Context as _, Result};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, ClipboardItem, Entity, Global,
    IntoElement, Render, SharedString, Subscription, Task, Window, WindowBounds, WindowKind,
    WindowOptions, div, point, px, size,
};
use gpui_shell::ScriptView;

use super::launcher;
use crate::{
    extensions::{
        CommandId, CommandMode, ExtensionHost, LaunchRequest, Opened, render_for_extension,
        take_menu_bar,
    },
    model::{Action, Effect, Image, MenuBarModel},
    pages::PageHandle,
};

/// A menu-bar command that is loaded, and its tray icon.
struct MenuBarRun {
    view: Entity<ScriptView>,
    /// The actions of the latest render, by the number the tray gave each.
    actions: Vec<Option<Action>>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    tray: Option<super::tray::Tray>,
    root: PathBuf,
    title: SharedString,
    fallback_icon: Image,
    _observe: Subscription,
}

#[derive(Default)]
struct Background {
    /// The commands the user turned on by opening them once; a bundled
    /// extension does not put an icon in everyone's tray on its own.
    active: BTreeSet<String>,
    window: Option<AnyWindowHandle>,
    menu_bars: HashMap<CommandId, MenuBarRun>,
    /// One timer per command with an `interval`, by command.
    timers: HashMap<CommandId, (Duration, Task<()>)>,
    _menu_events: Option<Task<()>>,
}

impl Global for Background {}

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Starts what the catalog asks to run on its own, and listens for tray menu
/// choices. Call once the launcher global exists.
pub fn start(cx: &mut App) {
    if cx.has_global::<Background>() {
        return;
    }
    let mut background = Background {
        active: load_active(),
        ..Default::default()
    };
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let (choices, chosen) = smol::channel::unbounded::<(String, Option<usize>)>();
        super::tray::menu_events(move |tray, number| {
            choices.try_send((tray, number)).ok();
        });
        background._menu_events = Some(cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok((tray, number)) = chosen.recv().await {
                cx.update(|cx| match number {
                    Some(number) => choose(&tray, number, cx),
                    None => deactivate_by_name(&tray, cx),
                });
            }
        }));
    }
    cx.set_global(background);
    sync(cx);
}

/// Matches what runs to the catalog: starts new menu-bar commands, stops
/// those that are gone, and keeps one timer per `interval`.
pub fn sync(cx: &mut App) {
    if !cx.has_global::<Background>() {
        return;
    }
    let Some((catalog, host)) = launcher::host_and_catalog(cx) else {
        return;
    };
    let active = cx.global::<Background>().active.clone();
    let wanted: Vec<(CommandId, CommandMode, Option<Duration>)> = catalog
        .commands()
        .filter(|(_, command)| {
            command.mode() != CommandMode::View && active.contains(&command.id().to_string())
        })
        .map(|(_, command)| (command.id().clone(), command.mode(), command.interval()))
        .collect();

    let gone: Vec<CommandId> = cx
        .global::<Background>()
        .menu_bars
        .keys()
        .filter(|id| {
            !wanted
                .iter()
                .any(|(command, mode, _)| command == *id && *mode == CommandMode::MenuBar)
        })
        .cloned()
        .collect();
    for id in gone {
        if let Some(run) = cx.global_mut::<Background>().menu_bars.remove(&id) {
            host.stop(&run.view, cx);
        }
    }

    let background = cx.global_mut::<Background>();
    background.timers.retain(|id, (interval, _)| {
        wanted
            .iter()
            .any(|(command, _, wanted)| command == id && *wanted == Some(*interval))
    });
    for (id, _, interval) in &wanted {
        let Some(interval) = *interval else {
            continue;
        };
        if cx.global::<Background>().timers.contains_key(id) {
            continue;
        }
        let command = id.clone();
        let timer = cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(interval).await;
                let command = command.clone();
                cx.update(|cx| {
                    if let Err(error) = run(LaunchRequest::new(command).in_background(), cx) {
                        tracing::warn!("{error:#}");
                    }
                });
            }
        });
        cx.global_mut::<Background>()
            .timers
            .insert(id.clone(), (interval, timer));
    }

    let start: Vec<CommandId> = wanted
        .iter()
        .filter(|(id, mode, _)| {
            *mode == CommandMode::MenuBar && !cx.global::<Background>().menu_bars.contains_key(id)
        })
        .map(|(id, _, _)| id.clone())
        .collect();
    for id in start {
        if let Err(error) = run(LaunchRequest::new(id).in_background(), cx) {
            tracing::warn!("{error:#}");
        }
    }
}

/// Stops the menu-bar commands of `extension` and starts them again from its
/// code as it is now.
pub fn restart_extension(extension: &str, cx: &mut App) {
    if !cx.has_global::<Background>() {
        return;
    }
    let stopped: Vec<CommandId> = cx
        .global::<Background>()
        .menu_bars
        .keys()
        .filter(|id| id.extension().as_ref() == extension)
        .cloned()
        .collect();
    for id in stopped {
        cx.global_mut::<Background>().menu_bars.remove(&id);
    }
    sync(cx);
}

/// Runs a `no-view` or `menu-bar` command in the background window. A
/// question the launcher must ask first (permission, preferences) comes back
/// as a page; a run the launcher started on its own leaves it unasked until
/// the user opens the command.
pub fn run(request: LaunchRequest, cx: &mut App) -> Result<Option<PageHandle>> {
    let (catalog, host) = launcher::host_and_catalog(cx).context("the launcher is not running")?;
    let window = window(cx)?;
    let id = request.command().clone();
    let (extension, command) = catalog
        .command(&id)
        .with_context(|| format!("no command `{id}`"))?;
    let opened = window.update(cx, |_, window, cx| {
        host.open(extension, command, &request, window, cx)
    })??;
    // Opening it is what turns a background command on.
    let activates = !request.is_background()
        && !matches!(opened, Opened::Page(_))
        && (command.mode() == CommandMode::MenuBar || command.interval().is_some());
    let result = match opened {
        Opened::Page(page) if request.is_background() => {
            tracing::info!(
                "`{id}` waits for the user to open it once: {}",
                page.title(cx)
            );
            Ok(None)
        }
        Opened::Page(page) => Ok(Some(page)),
        Opened::Background => Ok(None),
        Opened::MenuBar(view) => {
            let fallback_icon = command
                .icon()
                .map(|icon| Image::parse(icon))
                .unwrap_or_else(|| Image::Icon("app-window".into()));
            show_menu_bar(
                id.clone(),
                view,
                extension.root().to_path_buf(),
                command.title().clone(),
                fallback_icon,
                &host,
                cx,
            );
            Ok(None)
        }
    };
    if activates {
        activate(&id, cx);
    }
    result
}

/// Turns on a command the user opened, so the launcher runs it on its
/// `interval` (and, for a menu-bar command, from the next start) from now on.
pub fn activate(id: &CommandId, cx: &mut App) {
    if !cx.has_global::<Background>() {
        return;
    }
    let added = cx.global_mut::<Background>().active.insert(id.to_string());
    if added {
        save_active(&cx.global::<Background>().active);
        // Starts its timer.
        cx.defer(sync);
    }
}

/// Turns a command off: its tray icon goes, its timer stops, and it stays
/// off until the user opens it again.
pub fn deactivate(id: &CommandId, cx: &mut App) {
    if !cx.has_global::<Background>() {
        return;
    }
    let background = cx.global_mut::<Background>();
    if background.active.remove(&id.to_string()) {
        save_active(&background.active);
    }
    background.timers.remove(id);
    let stopped = background.menu_bars.remove(id);
    if let Some(run) = stopped
        && let Some((_, host)) = launcher::host_and_catalog(cx)
    {
        host.stop(&run.view, cx);
    }
}

fn deactivate_by_name(name: &str, cx: &mut App) {
    let id = cx
        .global::<Background>()
        .menu_bars
        .keys()
        .find(|id| id.to_string() == name)
        .cloned();
    if let Some(id) = id {
        deactivate(&id, cx);
    }
}

fn active_path() -> Option<PathBuf> {
    super::data_directory().map(|directory| directory.join("background-commands.json"))
}

fn load_active() -> BTreeSet<String> {
    active_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_active(active: &BTreeSet<String>) {
    let Some(path) = active_path() else {
        return;
    };
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::write(
                &path,
                serde_json::to_string_pretty(active).unwrap_or_default(),
            )
        });
    if let Err(error) = result {
        tracing::warn!("cannot save {}: {error}", path.display());
    }
}

fn show_menu_bar(
    id: CommandId,
    view: Entity<ScriptView>,
    root: PathBuf,
    title: SharedString,
    fallback_icon: Image,
    host: &Rc<ExtensionHost>,
    cx: &mut App,
) {
    let observed = id.clone();
    let _observe = cx.observe(&view, move |_, cx| {
        let id = observed.clone();
        // After the script call that changed it has returned.
        cx.defer(move |cx| render_menu_bar(&id, cx));
    });
    let run = MenuBarRun {
        view,
        actions: Vec::new(),
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        tray: None,
        root,
        title,
        fallback_icon,
        _observe,
    };
    if let Some(previous) = cx
        .global_mut::<Background>()
        .menu_bars
        .insert(id.clone(), run)
    {
        host.stop(&previous.view, cx);
        // The previous tray icon moves to the new run, so the icon does not
        // blink away and back on every refresh.
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(run) = cx.global_mut::<Background>().menu_bars.get_mut(&id) {
            run.tray = previous.tray;
        }
    }
    render_menu_bar(&id, cx);
}

/// Renders a menu-bar command and shows the result in its tray icon.
fn render_menu_bar(id: &CommandId, cx: &mut App) {
    let Some(view) = cx
        .global::<Background>()
        .menu_bars
        .get(id)
        .map(|run| run.view.clone())
    else {
        return;
    };
    let Ok(window) = window(cx) else {
        return;
    };
    let rendered = window.update(cx, |_, window, cx| {
        let extension: SharedString = view.read(cx).policy().application().to_owned().into();
        render_for_extension(&extension, || {
            view.update(cx, |view, cx| view.render_description(window, cx))
        })
    });
    let model = match rendered {
        Ok(Ok(mut element)) => take_menu_bar(&mut element),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(format!("{error:#}")),
    };
    let model = model.unwrap_or_else(|reason| {
        tracing::warn!("`{id}` could not show its menu: {reason}");
        MenuBarModel::new().with_tooltip(format!("{} failed: {reason}", id.command()))
    });
    let background = cx.global_mut::<Background>();
    let Some(run) = background.menu_bars.get_mut(id) else {
        return;
    };
    run.actions = super::tray::numbered_items(&model)
        .into_iter()
        .map(|item| item.action().cloned())
        .collect();
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let result = match &run.tray {
            Some(tray) => tray.update(&model, &run.fallback_icon, &run.root, &run.title),
            None => super::tray::Tray::new(
                &id.to_string(),
                &model,
                &run.fallback_icon,
                &run.root,
                &run.title,
            )
            .map(|tray| {
                run.tray = Some(tray);
            }),
        };
        if let Err(error) = result {
            tracing::warn!("cannot show the tray icon of `{id}`: {error:#}");
        }
    }
}

/// Performs the action of the entry chosen in a tray menu.
fn choose(tray: &str, number: usize, cx: &mut App) {
    let action = cx
        .global::<Background>()
        .menu_bars
        .iter()
        .find(|(id, _)| id.to_string() == tray)
        .and_then(|(_, run)| run.actions.get(number).cloned().flatten());
    if let Some(action) = action {
        perform(action.effect().clone(), cx);
    }
}

/// Carries out an effect without the launcher window: what a tray menu
/// entry or a background run asks for. What needs the window (a page, a
/// confirmation, pasting) shows the launcher and is done there.
pub fn perform(effect: Effect, cx: &mut App) {
    match effect {
        Effect::OpenUrl(url) => cx.open_url(&url),
        Effect::OpenPath(path) => cx.open_with_system(&path),
        Effect::RevealPath(path) => cx.reveal_path(&path),
        Effect::OpenWith {
            target,
            application,
        } => {
            if let Err(error) = super::platform::open_with(&target, &application) {
                super::platform::show_hud(format!("{error:#}").into(), cx);
            }
        }
        Effect::Copy(text) => {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
            super::platform::show_hud("Copied to clipboard".into(), cx);
        }
        Effect::CopyConcealed(text) => {
            crate::clipboard::copy_concealed(&text, cx);
            super::platform::show_hud("Copied to clipboard".into(), cx);
        }
        Effect::CopyItem(item) => {
            cx.write_to_clipboard(item);
            super::platform::show_hud("Copied to clipboard".into(), cx);
        }
        Effect::ShowHud(text) => super::platform::show_hud(text, cx),
        Effect::ShowToast(toast) => super::platform::show_hud(toast.title().clone(), cx),
        Effect::Trash(paths) => {
            cx.background_spawn(async move {
                if let Err(error) = super::platform::trash(&paths) {
                    tracing::warn!("{error:#}");
                }
            })
            .detach();
        }
        Effect::Run(handler) => {
            if let Ok(window) = window(cx) {
                window
                    .update(cx, |_, window, cx| handler.run(window, cx))
                    .ok();
            }
        }
        Effect::Launch(request) => launcher::open_command(request, cx),
        // Nothing to go back from, and nothing to close.
        Effect::Pop | Effect::PopToRoot | Effect::CloseWindow => {}
        effect => {
            launcher::show(cx);
            launcher::perform(effect, cx);
        }
    }
}

/// The background window, opened the first time it is needed.
fn window(cx: &mut App) -> Result<AnyWindowHandle> {
    let existing = cx
        .try_global::<Background>()
        .and_then(|background| background.window)
        .filter(|window| cx.windows().contains(window));
    if let Some(window) = existing {
        return Ok(window);
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(-10_000.), px(-10_000.)),
            size: size(px(1.), px(1.)),
        })),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("gpui-kit-launcher-background".into()),
        ..Default::default()
    };
    let (window, _) = gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Blank))?;
    let window: AnyWindowHandle = window;
    if cx.has_global::<Background>() {
        cx.global_mut::<Background>().window = Some(window);
    }
    Ok(window)
}
