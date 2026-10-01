//! Backing up and restoring a database with the database's own tools:
//! `pg_dump` and `pg_restore`/`psql` for PostgreSQL, `mysqldump` and `mysql`
//! for MySQL. SQLite needs no tool: `VACUUM INTO` writes a copy.
//!
//! The password is handed to the tool in its environment, never on the
//! command line, where other processes could read it.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};
use datakit_driver::ConnectionProfile;
use datakit_driver_mysql::MySqlDriver;
use datakit_driver_postgres::PostgresDriver;
use datakit_driver_sqlite::SqliteDriver;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonGroup, ButtonVariants as _},
    checkbox::Checkbox,
    dialog::{DialogAction, DialogClose, DialogFooter},
    form::{field, v_form},
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use rust_i18n::t;

use crate::{
    datasource::{DataSource, describe_error},
    services::Services,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Backup,
    Restore,
}

/// What the tools of one database are called.
struct Tools {
    backup: &'static str,
    restore: &'static str,
}

fn tools(driver: &str) -> Option<Tools> {
    match driver {
        PostgresDriver::ID => Some(Tools {
            backup: "pg_dump",
            restore: "psql",
        }),
        MySqlDriver::ID => Some(Tools {
            backup: "mysqldump",
            restore: "mysql",
        }),
        _ => None,
    }
}

/// `name` in a directory of `PATH`, or the bare name for the shell to find.
fn find_tool(name: &str) -> String {
    let executable = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|directory| directory.join(&executable))
                .find(|path| path.is_file())
        })
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| name.to_string())
}

/// What one run asks the tool to do.
struct Job {
    profile: ConnectionProfile,
    direction: Direction,
    tool: PathBuf,
    file: PathBuf,
    schema_only: bool,
    archive: bool,
    password: Option<String>,
}

/// Run `job` to the end and return what the tool printed.
fn run(job: Job) -> Result<String> {
    let profile = &job.profile;
    let mut command = Command::new(&job.tool);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    match profile.driver() {
        PostgresDriver::ID => {
            if let Some(password) = &job.password {
                command.env("PGPASSWORD", password);
            }
            command
                .arg("--host")
                .arg(profile.host())
                .arg("--port")
                .arg(profile.port().to_string())
                .arg("--username")
                .arg(profile.user())
                .arg("--no-password");
            let database = if profile.database().is_empty() {
                "postgres"
            } else {
                profile.database()
            };
            match job.direction {
                Direction::Backup => {
                    command
                        .arg("--dbname")
                        .arg(database)
                        .arg("--file")
                        .arg(&job.file)
                        .arg(if job.archive {
                            "--format=custom"
                        } else {
                            "--format=plain"
                        });
                    if job.schema_only {
                        command.arg("--schema-only");
                    }
                }
                Direction::Restore if job.archive => {
                    command
                        .arg("--dbname")
                        .arg(database)
                        .arg("--exit-on-error")
                        .arg(&job.file);
                }
                Direction::Restore => {
                    command
                        .arg("--dbname")
                        .arg(database)
                        .arg("--set")
                        .arg("ON_ERROR_STOP=1")
                        .arg("--file")
                        .arg(&job.file);
                }
            }
        }
        MySqlDriver::ID => {
            if let Some(password) = &job.password {
                command.env("MYSQL_PWD", password);
            }
            command
                .arg("--host")
                .arg(profile.host())
                .arg("--port")
                .arg(profile.port().to_string())
                .arg("--user")
                .arg(profile.user());
            match job.direction {
                Direction::Backup => {
                    command.arg("--result-file").arg(&job.file);
                    if job.schema_only {
                        command.arg("--no-data");
                    }
                    command.arg(profile.database());
                }
                Direction::Restore => {
                    command.arg(profile.database());
                    command
                        .stdin(Stdio::from(std::fs::File::open(&job.file).with_context(
                            || format!("Couldn’t open {}", job.file.display()),
                        )?));
                }
            }
        }
        driver => bail!("There is no backup tool for {driver} databases"),
    }
    let output = command
        .output()
        .with_context(|| format!("Couldn’t start {}", job.tool.display()))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        bail!(
            "{} failed ({}):\n{}",
            job.tool.display(),
            output.status,
            text.trim()
        );
    }
    Ok(text)
}

/// The backup dialog's content. Its footer is drawn by the dialog, apart
/// from this view, so every change of state also refreshes the window.
pub struct DumpDialog {
    data_source: Entity<DataSource>,
    direction: Direction,
    tool: Entity<InputState>,
    file: Entity<InputState>,
    schema_only: bool,
    archive: bool,
    output: Option<SharedString>,
    failed: bool,
    task: Option<Task<()>>,
}

impl DumpDialog {
    pub fn open(data_source: &Entity<DataSource>, window: &mut Window, cx: &mut App) {
        let name = data_source.read(cx).name();
        let data_source = data_source.clone();
        let dialog = cx.new(|cx| Self::new(data_source, window, cx));
        window.open_dialog(cx, {
            let dialog = dialog.clone();
            move |modal, _, cx| {
                let running = dialog.read(cx).task.is_some();
                modal
                    .title(t!("dump.title", name = name).to_string())
                    .w(px(620.))
                    .child(dialog.clone())
                    .footer(
                        DialogFooter::new().child(
                            h_flex()
                                .gap_2()
                                .child(
                                    DialogClose::new().child(
                                        Button::new("close")
                                            .outline()
                                            .label(t!("dump.close").to_string()),
                                    ),
                                )
                                .child(
                                    DialogAction::new().child(
                                        Button::new("run")
                                            .primary()
                                            .loading(running)
                                            .label(t!("dump.run").to_string()),
                                    ),
                                ),
                        ),
                    )
                    .on_ok({
                        let dialog = dialog.clone();
                        move |_, _, cx| {
                            dialog.update(cx, |dialog, cx| dialog.start(cx));
                            false
                        }
                    })
            }
        });
    }

    fn new(data_source: Entity<DataSource>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let driver = data_source.read(cx).profile().driver().to_string();
        let tool = tools(&driver)
            .map(|tools| find_tool(tools.backup))
            .unwrap_or_default();
        let suggested = dirs::document_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "{}.{}",
                data_source.read(cx).name(),
                if driver == SqliteDriver::ID {
                    "db"
                } else {
                    "sql"
                }
            ))
            .display()
            .to_string();
        Self {
            data_source,
            direction: Direction::Backup,
            tool: cx.new(|cx| InputState::new(window, cx).default_value(tool)),
            file: cx.new(|cx| InputState::new(window, cx).default_value(suggested)),
            schema_only: false,
            archive: false,
            output: None,
            failed: false,
            task: None,
        }
    }

    fn driver(&self, cx: &App) -> String {
        self.data_source.read(cx).profile().driver().to_string()
    }

    fn set_direction(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        self.direction = direction;
        if let Some(tools) = tools(&self.driver(cx)) {
            let name = match (direction, self.archive) {
                (Direction::Backup, _) => tools.backup,
                (Direction::Restore, true) => "pg_restore",
                (Direction::Restore, false) => tools.restore,
            };
            let tool = find_tool(name);
            self.tool
                .update(cx, |input, cx| input.set_value(tool, window, cx));
        }
        cx.notify();
        cx.refresh_windows();
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let current = PathBuf::from(self.file.read(cx).value().to_string());
        let directory = current
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        let file = self.file.clone();
        let picked = match self.direction {
            Direction::Backup => {
                let name = current
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string());
                let path = cx.prompt_for_new_path(&directory, name.as_deref());
                cx.spawn(async move |_, _| path.await.ok()?.ok()?)
            }
            Direction::Restore => {
                let paths = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: None,
                });
                cx.spawn(async move |_, _| paths.await.ok()?.ok()??.into_iter().next())
            }
        };
        cx.spawn(async move |_, cx| {
            let Some(path) = picked.await else {
                return;
            };
            let _ = cx.update(|cx| {
                let Some(window) = cx.active_window() else {
                    return;
                };
                let _ = window.update(cx, |_, window, cx| {
                    file.update(cx, |input, cx| {
                        input.set_value(path.display().to_string(), window, cx)
                    })
                });
            });
        })
        .detach();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        let profile = self.data_source.read(cx).profile().clone();
        let file = PathBuf::from(self.file.read(cx).value().trim().to_string());
        if profile.driver() == SqliteDriver::ID {
            self.backup_sqlite(file, cx);
            return;
        }
        let services = Services::global(cx);
        let secrets = services.secrets();
        let job_profile = profile.clone();
        let direction = self.direction;
        let tool = PathBuf::from(self.tool.read(cx).value().trim().to_string());
        let schema_only = self.schema_only;
        let archive = self.archive;
        self.output = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let password = secrets.read(job_profile.id())?;
                    run(Job {
                        profile: job_profile,
                        direction,
                        tool,
                        file,
                        schema_only,
                        archive,
                        password,
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.task = None;
                match result {
                    Ok(text) => {
                        this.failed = false;
                        let text = text.trim();
                        this.output = Some(if text.is_empty() {
                            t!("dump.done").into()
                        } else {
                            format!("{}\n{text}", t!("dump.done")).into()
                        });
                    }
                    Err(error) => {
                        this.failed = true;
                        this.output = Some(describe_error(&error));
                    }
                }
                cx.notify();
                cx.refresh_windows();
            });
        }));
        cx.notify();
        cx.refresh_windows();
    }

    /// SQLite writes a consistent copy of itself with `VACUUM INTO`.
    fn backup_sqlite(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        let target = file.display().to_string().replace('\'', "''");
        let task = self
            .data_source
            .read(cx)
            .run_reading_statements(vec![format!("VACUUM INTO '{target}'")], cx);
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.task = None;
                this.failed = result.is_err();
                this.output = Some(match result {
                    Ok(()) => t!("dump.done").into(),
                    Err(error) => describe_error(&error),
                });
                cx.notify();
                cx.refresh_windows();
            });
        }));
        cx.notify();
        cx.refresh_windows();
    }
}

impl Render for DumpDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let driver = self.driver(cx);
        let sqlite = driver == SqliteDriver::ID;
        let supported = sqlite || tools(&driver).is_some();
        let postgres = driver == PostgresDriver::ID;
        if !supported {
            return div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t!("dump.unsupported").to_string())
                .into_any_element();
        }
        v_flex()
            .gap_3()
            .child(
                ButtonGroup::new("dump-direction")
                    .small()
                    .outline()
                    .child(
                        Button::new("backup")
                            .label(t!("dump.backup").to_string())
                            .selected(self.direction == Direction::Backup),
                    )
                    .when(!sqlite, |group| {
                        group.child(
                            Button::new("restore")
                                .label(t!("dump.restore").to_string())
                                .selected(self.direction == Direction::Restore),
                        )
                    })
                    .on_click(cx.listener(|this, selected: &Vec<usize>, window, cx| {
                        let direction = if selected.contains(&1) {
                            Direction::Restore
                        } else {
                            Direction::Backup
                        };
                        this.set_direction(direction, window, cx);
                    })),
            )
            .child(
                v_form()
                    .when(!sqlite, |form| {
                        form.child(
                            field()
                                .label(t!("dump.tool").to_string())
                                .child(Input::new(&self.tool)),
                        )
                    })
                    .child(
                        field().label(t!("dump.file").to_string()).child(
                            h_flex().gap_2().child(Input::new(&self.file)).child(
                                Button::new("browse-file")
                                    .outline()
                                    .icon(IconName::FolderOpen)
                                    .label(t!("datasource.form.browse").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
                            ),
                        ),
                    ),
            )
            .when(!sqlite, |body| {
                body.child(
                    h_flex()
                        .gap_4()
                        .when(postgres, |row| {
                            row.child(
                                Checkbox::new("archive")
                                    .label(t!("dump.archive").to_string())
                                    .checked(self.archive)
                                    .on_click(cx.listener(|this, checked: &bool, window, cx| {
                                        this.archive = *checked;
                                        let direction = this.direction;
                                        this.set_direction(direction, window, cx);
                                    })),
                            )
                        })
                        .when(self.direction == Direction::Backup, |row| {
                            row.child(
                                Checkbox::new("schema-only")
                                    .label(t!("dump.schema_only").to_string())
                                    .checked(self.schema_only)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.schema_only = *checked;
                                        cx.notify();
                                        cx.refresh_windows();
                                    })),
                            )
                        }),
                )
            })
            .when_some(self.output.clone(), |body, output| {
                body.child(
                    div()
                        .id("dump-output")
                        .max_h(px(220.))
                        .overflow_y_scrollbar()
                        .p_2()
                        .rounded(theme.radius)
                        .bg(theme.muted)
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .when(self.failed, |output| output.text_color(theme.danger))
                        .child(output),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .when(!sqlite, |note| {
                        note.child(t!("dump.password_note").to_string())
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_tool_is_left_for_the_shell_to_find() {
        assert_eq!(
            find_tool("datakit-surely-not-a-tool"),
            "datakit-surely-not-a-tool"
        );
    }

    #[test]
    fn only_server_databases_have_tools() {
        assert!(tools(PostgresDriver::ID).is_some());
        assert!(tools(MySqlDriver::ID).is_some());
        assert!(tools(SqliteDriver::ID).is_none());
    }
}
