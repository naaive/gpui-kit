//! Running script commands: their root search rows, the arguments form, the
//! live output page, and creating a new script.

use std::{
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};

use super::{Mode, ScriptCommand, directory, interpreter};
use crate::{
    format::code_block,
    model::{
        Accessory, Action, ActionPanel, Choice, Control, DetailModel, Effect, Field, FormHandler,
        FormModel, FormValue, FormValues, Image, Item, ItemId, Metadata, MetadataValue, PageModel,
        PushHandler, RunHandler, Toast, ToastStyle,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
    sources::applications::REVEAL_TITLE,
};

/// The output page keeps the end of very long output.
const MAX_OUTPUT: usize = 64 * 1024;

/// One row per script command, for the root search.
pub fn script_items(commands: &[ScriptCommand]) -> Vec<Item> {
    commands.iter().map(item).collect()
}

fn item(command: &ScriptCommand) -> Item {
    let image = match &command.icon {
        Some(icon) if icon.contains('.') => {
            let path = Path::new(icon);
            let path = match path.is_absolute() {
                true => path.to_path_buf(),
                false => command
                    .path
                    .parent()
                    .map(|parent| parent.join(path))
                    .unwrap_or_else(|| path.to_path_buf()),
            };
            Image::File(path)
        }
        Some(icon)
            if icon
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()) =>
        {
            Image::Icon(icon.clone().into())
        }
        // An emoji, or a few characters.
        Some(icon) if icon.chars().count() <= 2 => Image::Glyph(icon.clone().into()),
        _ => Image::Icon("square-terminal".into()),
    };
    let run = command.clone();
    let item = Item::new(
        ItemId::new(format!(
            "script/{}",
            command
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
        command.title.clone(),
    )
    .with_image(image)
    .with_accessory(Accessory::text("Script Command"))
    .with_action(
        Action::new(
            "Run Script",
            match command.arguments.is_empty() {
                true => Effect::Run(RunHandler::new(move |(), window, cx| {
                    start(&run, Vec::new(), window, cx)
                })),
                false => Effect::Push(PushHandler::new(move |_, cx| {
                    Ok(pages::handle(cx.new(|_| ArgumentsPage {
                        command: run.clone(),
                    })))
                })),
            },
        )
        .with_image(Image::Icon("play".into())),
    )
    .with_action(
        Action::new(REVEAL_TITLE, Effect::RevealPath(command.path.clone()))
            .with_image(Image::Icon("folder-open".into()))
            .with_shortcut("secondary-shift-f"),
    )
    .with_action(
        Action::new(
            "Copy Script Path",
            Effect::Copy(command.path.display().to_string().into()),
        )
        .with_image(Image::Icon("copy".into()))
        .with_shortcut("secondary-shift-c"),
    );
    let item = match &command.package {
        Some(package) => item.with_subtitle(package.clone()),
        None => item,
    };
    match &command.description {
        Some(description) => item.with_keyword(description.clone()),
        None => item,
    }
}

/// Runs `command` with `arguments` the way its mode asks.
fn start(command: &ScriptCommand, arguments: Vec<String>, _: &mut Window, cx: &mut App) {
    match command.mode {
        Mode::FullOutput => {
            let command = command.clone();
            perform(
                Effect::Push(PushHandler::new(move |_, cx| {
                    let command = command.clone();
                    let arguments = arguments.clone();
                    Ok(pages::handle(cx.new(|cx| {
                        let mut page = OutputPage {
                            command,
                            arguments,
                            output: String::new(),
                            status: None,
                            started: Instant::now(),
                            child: None,
                            tasks: Vec::new(),
                        };
                        page.run(cx);
                        page
                    })))
                })),
                cx,
            );
        }
        Mode::Compact | Mode::Silent => {
            let silent = command.mode == Mode::Silent;
            if silent {
                perform(Effect::CloseWindow, cx);
            }
            let (command, title) = (command.clone(), command.title.clone());
            let task = cx.background_spawn(async move { run_to_end(&command, &arguments) });
            cx.spawn(async move |cx| {
                let result = task.await;
                cx.update(|cx| {
                    let effect = match (result, silent) {
                        (Ok((true, output)), true) => {
                            Effect::ShowHud(last_line(&output, &title).into())
                        }
                        (Ok((true, output)), false) => Effect::ShowToast(Toast::new(
                            ToastStyle::Success,
                            last_line(&output, &title),
                        )),
                        (Ok((false, output)), _) => Effect::ShowToast(
                            Toast::new(ToastStyle::Failure, format!("“{title}” failed"))
                                .with_message(last_line(&output, "")),
                        ),
                        (Err(error), _) => Effect::ShowToast(
                            Toast::new(ToastStyle::Failure, format!("Couldn’t run “{title}”"))
                                .with_message(format!("{error:#}")),
                        ),
                    };
                    perform(effect, cx);
                });
            })
            .detach();
        }
    }
}

fn last_line(output: &str, fallback: &str) -> String {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

/// The process for `command`, with stdout and stderr piped.
fn spawn(command: &ScriptCommand, arguments: &[String]) -> Result<std::process::Child> {
    let first_line = std::fs::read_to_string(&command.path)
        .ok()
        .and_then(|source| source.lines().next().map(str::to_owned))
        .unwrap_or_default();
    let program = interpreter(&command.path, &first_line);
    let mut process = match program.split_first() {
        Some((program, rest)) => {
            let mut process = Command::new(program);
            process.args(rest).arg(&command.path);
            process
        }
        None => Command::new(&command.path),
    };
    let directory = command
        .current_directory
        .clone()
        .or_else(|| command.path.parent().map(Path::to_path_buf));
    if let Some(directory) = directory {
        process.current_dir(directory);
    }
    process
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    process.spawn().with_context(|| match program.first() {
        Some(program) => format!("cannot start `{program}`; is it installed?"),
        None => "cannot start the script; give it a known extension or a #! line".into(),
    })
}

/// Runs to the end; whether it succeeded, and its output.
fn run_to_end(command: &ScriptCommand, arguments: &[String]) -> Result<(bool, String)> {
    let output = spawn(command, arguments)?.wait_with_output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.success(), text))
}

/// Shows a full-output script's output as it arrives.
struct OutputPage {
    command: ScriptCommand,
    arguments: Vec<String>,
    output: String,
    /// The exit code once the script ended, or why it could not start.
    status: Option<Result<Option<i32>, String>>,
    started: Instant,
    /// The running script, ended when the page closes or runs it again.
    child: Option<Arc<Mutex<Child>>>,
    tasks: Vec<Task<()>>,
}

impl Drop for OutputPage {
    fn drop(&mut self) {
        self.stop();
    }
}

impl OutputPage {
    fn stop(&mut self) {
        if let Some(child) = self.child.take()
            && let Ok(mut child) = child.lock()
        {
            child.kill().ok();
        }
        self.tasks.clear();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        self.stop();
        self.output.clear();
        self.status = None;
        self.started = Instant::now();
        let child = spawn(&self.command, &self.arguments);
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                self.status = Some(Err(format!("{error:#}")));
                return;
            }
        };
        let (lines, received) = smol::channel::unbounded::<String>();
        for stream in [
            child
                .stdout
                .take()
                .map(|out| Box::new(out) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|err| Box::new(err) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let lines = lines.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if lines.send_blocking(line).is_err() {
                        break;
                    }
                }
            });
        }
        drop(lines);
        let child = Arc::new(Mutex::new(child));
        self.child = Some(child.clone());
        // Polled rather than awaited, so the page can kill the script; and
        // apart from the output, which a background process the script left
        // behind may keep open after the script itself has ended.
        let wait = cx.background_spawn(async move {
            loop {
                match child.lock().map(|mut child| child.try_wait()) {
                    Ok(Ok(Some(status))) => return status.code(),
                    Ok(Ok(None)) => {}
                    Ok(Err(_)) | Err(_) => return None,
                }
                smol::Timer::after(Duration::from_millis(100)).await;
            }
        });
        self.tasks.push(cx.spawn(async move |this, cx| {
            let code = wait.await;
            this.update(cx, |page, cx| {
                page.status = Some(Ok(code));
                cx.notify();
            })
            .ok();
        }));
        self.tasks.push(cx.spawn(async move |this, cx| {
            while let Ok(line) = received.recv().await {
                let alive = this
                    .update(cx, |page, cx| {
                        page.output.push_str(&line);
                        page.output.push('\n');
                        if page.output.len() > MAX_OUTPUT {
                            let cut = page.output.len() - MAX_OUTPUT;
                            let cut = (cut..page.output.len())
                                .find(|ix| page.output.is_char_boundary(*ix))
                                .unwrap_or(cut);
                            page.output.drain(..cut);
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    return;
                }
            }
        }));
    }
}

impl Page for OutputPage {
    fn title(&self) -> SharedString {
        self.command.title.clone().into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let running = self.status.is_none();
        let body = match &self.status {
            Some(Err(error)) => format!("**Couldn’t run the script.** {error}"),
            _ if self.output.trim().is_empty() && running => String::new(),
            _ if self.output.trim().is_empty() => "*No output.*".into(),
            _ => code_block(self.output.trim_end()),
        };
        let label =
            |label: &str, value: String| Metadata::new(label, MetadataValue::Text(value.into()));
        let state = match &self.status {
            None => "Running…".to_owned(),
            Some(Ok(Some(0))) => "Succeeded".to_owned(),
            Some(Ok(Some(code))) => format!("Failed with exit code {code}"),
            Some(Ok(None)) => "Stopped".to_owned(),
            Some(Err(_)) => "Didn’t start".to_owned(),
        };
        DetailModel::new(body)
            .with_loading(running)
            .with_metadata(label("Status", state))
            .with_metadata(label(
                "Duration",
                format!("{:.1} s", self.started.elapsed().as_secs_f32()),
            ))
            .with_metadata(label("Script", self.command.path.display().to_string()))
            .with_actions(
                ActionPanel::new()
                    .with_action(Action::new(
                        "Copy Output",
                        Effect::Copy(self.output.trim_end().to_owned().into()),
                    ))
                    .with_action(
                        Action::new(
                            "Run Again",
                            Effect::Run(RunHandler::new(move |(), _, cx| {
                                page.update(cx, |page, cx| {
                                    page.run(cx);
                                    cx.notify();
                                })
                                .ok();
                            })),
                        )
                        .with_shortcut("secondary-r"),
                    ),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

/// Asks for a script's arguments, then runs it.
struct ArgumentsPage {
    command: ScriptCommand,
}

impl Page for ArgumentsPage {
    fn title(&self) -> SharedString {
        self.command.title.clone().into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let command = self.command.clone();
        let count = command.arguments.len();
        let submit = FormHandler::new(move |values, window, cx| {
            let arguments: Vec<String> = (0..count)
                .map(|ix| text(&values, &format!("argument{ix}")))
                .collect();
            if let Some(ix) = command
                .arguments
                .iter()
                .zip(&arguments)
                .position(|(argument, value)| !argument.optional && value.is_empty())
            {
                perform(
                    Effect::ShowToast(Toast::new(
                        ToastStyle::Failure,
                        format!("Fill in “{}”", command.arguments[ix].placeholder),
                    )),
                    cx,
                );
                return;
            }
            perform(Effect::Pop, cx);
            start(&command, arguments, window, cx);
        });
        self.command
            .arguments
            .iter()
            .enumerate()
            .fold(
                FormModel::new().with_actions(
                    ActionPanel::new()
                        .with_action(Action::new("Run Script", Effect::SubmitForm(submit))),
                ),
                |form, (ix, argument)| {
                    let placeholder: SharedString = argument.placeholder.clone().into();
                    let title = match argument.optional {
                        true => format!("{} (optional)", argument.placeholder),
                        false => argument.placeholder.clone(),
                    };
                    let control = match argument.kind.as_str() {
                        "password" => Control::Password {
                            placeholder: Some(placeholder),
                            value: SharedString::default(),
                        },
                        _ => Control::Text {
                            placeholder: Some(placeholder),
                            value: SharedString::default(),
                        },
                    };
                    form.with_field(Field::new(format!("argument{ix}"), title, control))
                },
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

fn text(values: &FormValues, id: &str) -> String {
    match values.get(id) {
        Some(FormValue::Text(text)) => text.to_string(),
        Some(FormValue::Empty | FormValue::Bool(_) | FormValue::List(_)) | None => String::new(),
    }
}

/// The languages a new script can be written in: title, extension, the
/// comment marker, and the line that prints.
const LANGUAGES: &[(&str, &str, &str, &str)] = &[
    (
        "PowerShell",
        "ps1",
        "#",
        "Write-Output \"Hello from {title}\"",
    ),
    ("Python", "py", "#", "print(\"Hello from {title}\")"),
    ("Bash", "sh", "#", "echo \"Hello from {title}\""),
    (
        "Node.js",
        "js",
        "//",
        "console.log(\"Hello from {title}\");",
    ),
];

/// The Create Script Command form.
pub fn create_script_command(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|_| CreateScriptPage { error: None })))
}

struct CreateScriptPage {
    error: Option<String>,
}

impl CreateScriptPage {
    fn create(values: &FormValues) -> Result<PathBuf> {
        let title = text(values, "title").trim().to_owned();
        if title.is_empty() {
            return Err(anyhow!("Give the command a title"));
        }
        let language = text(values, "language");
        let (_, extension, comment, body) = LANGUAGES
            .iter()
            .find(|(name, ..)| *name == language)
            .copied()
            .unwrap_or(LANGUAGES[0]);
        let mode = match text(values, "mode").as_str() {
            "" => "fullOutput".to_owned(),
            mode => mode.to_owned(),
        };
        let slug: String = title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let slug = match slug.is_empty() {
            true => "script-command".to_owned(),
            false => slug,
        };
        // The title goes into the script's greeting; quotes and `$` would
        // end or expand the string in some of the languages.
        let greeting: String = title
            .chars()
            .filter(|c| !matches!(c, '"' | '\'' | '$' | '`' | '\\'))
            .collect();
        let directory = directory().ok_or_else(|| anyhow!("no data folder"))?;
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{slug}.{extension}"));
        if path.exists() {
            return Err(anyhow!("{} already exists", path.display()));
        }
        let shebang = match extension {
            "sh" => "#!/bin/bash\n\n",
            "py" => "#!/usr/bin/env python3\n\n",
            "js" => "#!/usr/bin/env node\n\n",
            _ => "",
        };
        let source = format!(
            "{shebang}{comment} @raycast.schemaVersion 1\n{comment} @raycast.title {title}\n\
             {comment} @raycast.mode {mode}\n{comment} @raycast.packageName Scripts\n\n{}\n",
            body.replace("{title}", &greeting)
        );
        std::fs::write(&path, source)?;
        Ok(path)
    }
}

impl Page for CreateScriptPage {
    fn title(&self) -> SharedString {
        "Create Script Command".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let submit =
            FormHandler::new(
                move |values, _, cx| match CreateScriptPage::create(&values) {
                    Ok(path) => {
                        perform(Effect::Pop, cx);
                        perform(
                            Effect::ShowToast(
                                Toast::new(ToastStyle::Success, "Script command created")
                                    .with_message(path.display().to_string()),
                            ),
                            cx,
                        );
                        perform(Effect::RevealPath(path), cx);
                    }
                    Err(error) => {
                        page.update(cx, |page, cx| {
                            page.error = Some(format!("{error:#}"));
                            cx.notify();
                        })
                        .ok();
                    }
                },
            );
        let title = Field::new(
            "title",
            "Title",
            Control::Text {
                placeholder: Some("Say Hello".into()),
                value: SharedString::default(),
            },
        )
        .with_info(match directory() {
            Some(directory) => format!("Saved in {}", directory.display()),
            None => String::new(),
        });
        let title = match &self.error {
            Some(error) => title.with_error(error.clone()),
            None => title,
        };
        FormModel::new()
            .with_field(title)
            .with_field(Field::new(
                "language",
                "Language",
                Control::Dropdown {
                    choices: LANGUAGES
                        .iter()
                        .map(|(name, ..)| Choice::new(*name, *name))
                        .collect(),
                    value: Some(LANGUAGES[0].0.into()),
                },
            ))
            .with_field(Field::new(
                "mode",
                "Output",
                Control::Dropdown {
                    choices: vec![
                        Choice::new("fullOutput", "Show on a page"),
                        Choice::new("compact", "Show as a toast"),
                        Choice::new("silent", "Hide the launcher, show a HUD"),
                    ],
                    value: Some("fullOutput".into()),
                },
            ))
            .with_actions(ActionPanel::new().with_action(Action::new(
                "Create Script Command",
                Effect::SubmitForm(submit),
            )))
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_script_runs_with_its_arguments() {
        let root = tempfile::tempdir().unwrap();
        let (name, source) = match cfg!(windows) {
            true => (
                "hello.ps1",
                "# @raycast.title Hello\n# @raycast.mode compact\nWrite-Output \"hello $($args[0])\"\n",
            ),
            false => (
                "hello.sh",
                "#!/bin/sh\n# @raycast.title Hello\n# @raycast.mode compact\necho \"hello $1\"\n",
            ),
        };
        std::fs::write(root.path().join(name), source).unwrap();
        let commands = super::super::discover(root.path());
        assert_eq!(commands.len(), 1);
        let (ok, output) = run_to_end(&commands[0], &["world".into()]).unwrap();
        assert!(ok, "{output}");
        assert_eq!(last_line(&output, ""), "hello world");
    }
}
