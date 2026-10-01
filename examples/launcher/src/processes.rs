//! Search Processes: running programs with their CPU and memory use, to
//! quit or force quit — Raycast's Quit Application and Kill Process in one.
//!
//! Processes are grouped by program, since one browser runs dozens. The list
//! refreshes every few seconds while it is open; CPU use needs two samples,
//! so it appears from the second refresh.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};
use sysinfo::{ProcessesToUpdate, System};

use crate::{
    format::format_bytes,
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation,
        Effect, Image, Item, ItemId, ListModel, PageModel, RunHandler, Section, Toast, ToastStyle,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
    sources::applications::{REVEAL_TITLE, file_icon},
};

const REFRESH: Duration = Duration::from_secs(2);

/// One program and every process running it.
#[derive(Clone, Debug)]
struct Program {
    name: String,
    exe: Option<PathBuf>,
    pids: Vec<u32>,
    memory: u64,
    cpu: f32,
    icon: Option<PathBuf>,
}

impl Program {
    fn title(&self) -> String {
        self.name
            .strip_suffix(".exe")
            .unwrap_or(&self.name)
            .to_owned()
    }
}

pub fn search_processes_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|cx| {
        let mut page = ProcessesPage {
            programs: Vec::new(),
            loaded: false,
            system: Arc::new(Mutex::new(System::new())),
            icons: Arc::new(Mutex::new(HashMap::new())),
            task: None,
        };
        page.watch(cx);
        page
    })))
}

struct ProcessesPage {
    programs: Vec<Program>,
    loaded: bool,
    /// Kept between samples, which is how CPU use is measured.
    system: Arc<Mutex<System>>,
    /// Icons by executable, so each is looked up once.
    icons: Arc<Mutex<HashMap<PathBuf, Option<PathBuf>>>>,
    task: Option<Task<()>>,
}

impl ProcessesPage {
    /// Samples now and every few seconds until the page closes.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let (system, icons) = (self.system.clone(), self.icons.clone());
        self.task = Some(cx.spawn(async move |this, cx| {
            loop {
                let (system, icons) = (system.clone(), icons.clone());
                let programs = cx
                    .background_spawn(async move { sample(&system, &icons) })
                    .await;
                let alive = this
                    .update(cx, |page, cx| {
                        page.programs = programs;
                        page.loaded = true;
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        }));
    }
}

fn sample(
    system: &Mutex<System>,
    icons: &Mutex<HashMap<PathBuf, Option<PathBuf>>>,
) -> Vec<Program> {
    let Ok(mut system) = system.lock() else {
        return Vec::new();
    };
    system.refresh_processes(ProcessesToUpdate::All, true);
    let own = std::process::id();
    let mut programs: HashMap<String, Program> = HashMap::new();
    for (pid, process) in system.processes() {
        let pid = pid.as_u32();
        // The launcher itself, and the idle and kernel pseudo-processes.
        if pid == own || pid <= 4 {
            continue;
        }
        let name = process.name().to_string_lossy().into_owned();
        if name.is_empty() {
            continue;
        }
        let program = programs
            .entry(name.to_lowercase())
            .or_insert_with(|| Program {
                name: name.clone(),
                exe: process.exe().map(PathBuf::from),
                pids: Vec::new(),
                memory: 0,
                cpu: 0.,
                icon: None,
            });
        program.pids.push(pid);
        program.memory += process.memory();
        program.cpu += process.cpu_usage();
    }
    drop(system);

    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get()) as f32;
    let mut programs: Vec<Program> = programs.into_values().collect();
    for program in &mut programs {
        // Per core in sysinfo; per machine, as task managers show it.
        program.cpu /= cores;
        if let Some(exe) = &program.exe
            && let Ok(mut icons) = icons.lock()
        {
            program.icon = icons
                .entry(exe.clone())
                .or_insert_with(|| file_icon(exe))
                .clone();
        }
    }
    programs.sort_by_key(|program| std::cmp::Reverse(program.memory));
    programs
}

/// Ends the program's processes: politely (`taskkill` without `/F` asks
/// its windows to close) or by force.
fn quit(program: &Program, force: bool, cx: &mut App) {
    let pids = program.pids.clone();
    let title = program.title();
    let task = cx.background_spawn(async move { end(&pids, force) });
    cx.spawn(async move |cx| {
        let ended = task.await;
        cx.update(|cx| {
            let toast = match ended {
                true => Toast::new(
                    ToastStyle::Success,
                    match force {
                        true => format!("Force quit {title}"),
                        false => format!("Asked {title} to quit"),
                    },
                ),
                false => Toast::new(ToastStyle::Failure, format!("Couldn’t quit {title}"))
                    .with_message("It may belong to another user or to the system."),
            };
            perform(Effect::ShowToast(toast), cx);
        });
    })
    .detach();
}

#[cfg(target_os = "windows")]
fn end(pids: &[u32], force: bool) -> bool {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new("taskkill");
    if force {
        command.arg("/F");
    }
    for pid in pids {
        command.arg("/PID").arg(pid.to_string());
    }
    command
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(target_os = "windows"))]
fn end(pids: &[u32], force: bool) -> bool {
    let signal = match force {
        true => "-KILL",
        false => "-TERM",
    };
    std::process::Command::new("kill")
        .arg(signal)
        .args(pids.iter().map(ToString::to_string))
        .status()
        .is_ok_and(|status| status.success())
}

fn item(program: &Program) -> Item {
    let title = program.title();
    let image = match &program.icon {
        Some(icon) => Image::File(icon.clone()),
        None => Image::Icon("cpu".into()),
    };
    let polite = program.clone();
    let forced = program.clone();
    let pids = program
        .pids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut actions = ActionPanel::new()
        .with_action(
            Action::new(
                format!("Quit {title}"),
                Effect::Run(RunHandler::new(move |(), _, cx| quit(&polite, false, cx))),
            )
            .with_image(Image::Icon("circle-x".into())),
        )
        .with_action(
            Action::new(
                format!("Force Quit {title}"),
                Effect::Confirm(
                    Confirmation::new(
                        format!("Force quit {title}?"),
                        Effect::Run(RunHandler::new(move |(), _, cx| quit(&forced, true, cx))),
                    )
                    .with_message("Unsaved work in it is lost.")
                    .with_confirm_title("Force Quit")
                    .destructive(true),
                ),
            )
            .with_image(Image::Icon("skull".into()))
            .with_style(ActionStyle::Destructive),
        )
        .with_action(
            Action::new("Copy Process ID", Effect::Copy(pids.into()))
                .with_image(Image::Icon("hash".into()))
                .with_shortcut("secondary-shift-c"),
        );
    if let Some(exe) = &program.exe {
        actions = actions.with_section(
            ActionSection::new()
                .with_entry(ActionEntry::Action(
                    Action::new(REVEAL_TITLE, Effect::RevealPath(exe.clone()))
                        .with_image(Image::Icon("folder-open".into()))
                        .with_shortcut("secondary-shift-f"),
                ))
                .with_entry(ActionEntry::Action(
                    Action::new("Copy Path", Effect::Copy(exe.display().to_string().into()))
                        .with_image(Image::Icon("copy".into())),
                )),
        );
    }
    let item = Item::new(
        ItemId::new(format!("process/{}", program.name.to_lowercase())),
        title,
    )
    .with_image(image)
    .with_keyword(program.name.clone())
    .with_accessory(Accessory::text(format!("{:.1}%", program.cpu)).with_tooltip("CPU"))
    .with_accessory(Accessory::text(format_bytes(program.memory)).with_tooltip("Memory"))
    .with_actions(actions);
    match program.pids.len() {
        1 => item.with_subtitle(format!("PID {}", program.pids[0])),
        count => item.with_subtitle(format!("{count} processes")),
    }
}

impl Page for ProcessesPage {
    fn title(&self) -> SharedString {
        "Processes".into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        ListModel::new()
            .with_placeholder("Search running programs…")
            .with_loading(!self.loaded)
            .with_empty_title(match self.loaded {
                true => "No matching programs",
                false => "Reading running programs…",
            })
            .with_section(
                Section::new()
                    .with_title("Programs")
                    .with_subtitle(format!("{}", self.programs.len()))
                    .with_items(self.programs.iter().map(item)),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
