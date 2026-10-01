use std::{path::PathBuf, sync::Arc};

use datakit_driver::{ConnectionProfile, DataSourceId, Driver, SslMode};
use datakit_driver_postgres::PostgresDriver;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    dialog::{DialogAction, DialogClose, DialogFooter},
    form::{field, v_form},
    h_flex,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectItem, SelectState},
    spinner::Spinner,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions,
    Render, SharedString, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, rems,
};
use rust_i18n::t;

use super::{DataSource, DataSources, describe_error, open_connection, tunnel};
use crate::services::Services;

/// The fields of a data source, for adding one or changing its properties.
pub struct DataSourceForm {
    /// The data source being edited, or `None` for a new one.
    existing: Option<DataSourceId>,
    driver: Entity<SelectState<Vec<DriverChoice>>>,
    /// The driver the fields are set up for.
    driver_id: &'static str,
    name: Entity<InputState>,
    file: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    database: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    ssl_mode: Entity<SelectState<Vec<SslChoice>>>,
    use_ssh: bool,
    ssh_host: Entity<InputState>,
    ssh_port: Entity<InputState>,
    ssh_user: Entity<InputState>,
    ssh_password: Entity<InputState>,
    ssh_key_file: Entity<InputState>,
    port_error: Option<SharedString>,
    test: TestStatus,
    browse_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

enum TestStatus {
    Idle,
    /// Holds the test so closing the form abandons it.
    Testing {
        _task: Task<()>,
    },
    Succeeded(SharedString),
    Failed(SharedString),
}

#[derive(Clone)]
struct SslChoice {
    mode: SslMode,
    title: SharedString,
}

impl SelectItem for SslChoice {
    type Value = SslMode;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &SslMode {
        &self.mode
    }
}

#[derive(Clone)]
struct DriverChoice {
    id: &'static str,
    title: SharedString,
}

impl SelectItem for DriverChoice {
    type Value = &'static str;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &&'static str {
        &self.id
    }
}

impl DataSourceForm {
    /// Open the form in a dialog: empty for a new data source, or filled in
    /// from `existing`.
    pub fn open(existing: Option<&Entity<DataSource>>, window: &mut Window, cx: &mut App) {
        let profile = existing.map(|data_source| data_source.read(cx).profile().clone());
        let is_new = profile.is_none();
        let form = cx.new(|cx| Self::new(profile, window, cx));
        let title: SharedString = if is_new {
            t!("datasource.form.new_title").into()
        } else {
            t!("datasource.form.edit_title").into()
        };
        let width = rems(34.).to_pixels(window.rem_size());
        window.open_dialog(cx, {
            let form = form.clone();
            move |dialog, _, cx| {
                dialog
                    .title(title.clone())
                    .w(width)
                    .child(form.clone())
                    .footer(form.read(cx).render_footer(&form, cx))
                    .on_ok({
                        let form = form.clone();
                        move |_, window, cx| form.update(cx, |form, cx| form.save(window, cx))
                    })
            }
        });
        let name = form.read(cx).name.clone();
        name.update(cx, |input, cx| input.focus(window, cx));
    }

    fn new(
        profile: Option<ConnectionProfile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let drivers: Vec<Arc<dyn Driver>> = Services::global(cx).drivers().drivers().to_vec();
        let defaults = ConnectionProfile::new(PostgresDriver::ID, 5432).with_user("postgres");
        let existing = profile.as_ref().map(|profile| profile.id().clone());
        let profile = profile.unwrap_or(defaults);
        let input = |value: &str,
                     placeholder: SharedString,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            let value = SharedString::from(value.to_string());
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let option = |name: &str| profile.option(name).unwrap_or_default().to_string();
        let name = input(profile.given_name(), SharedString::default(), window, cx);
        let file = input(
            profile.option(ConnectionProfile::FILE).unwrap_or_default(),
            t!("datasource.form.file_placeholder").into(),
            window,
            cx,
        );
        let host = input(profile.host(), "localhost".into(), window, cx);
        let port = input(&profile.port().to_string(), "5432".into(), window, cx);
        let database = input(profile.database(), SharedString::default(), window, cx);
        let user = input(profile.user(), SharedString::default(), window, cx);
        let password_placeholder: SharedString = if existing.is_some() {
            t!("datasource.form.password_unchanged").into()
        } else {
            SharedString::default()
        };
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(password_placeholder.clone())
        });
        let ssh_host = input(&option(tunnel::HOST), SharedString::default(), window, cx);
        let ssh_port = input(
            profile.option(tunnel::PORT).unwrap_or("22"),
            "22".into(),
            window,
            cx,
        );
        let ssh_user = input(&option(tunnel::USER), SharedString::default(), window, cx);
        let ssh_password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(password_placeholder)
        });
        let ssh_key_file = input(
            &option(tunnel::KEY_FILE),
            t!("datasource.form.ssh_key_placeholder").into(),
            window,
            cx,
        );
        let ssl_choices: Vec<SslChoice> = SslMode::ALL
            .iter()
            .map(|mode| SslChoice {
                mode: *mode,
                title: ssl_mode_title(*mode),
            })
            .collect();
        let selected = SslMode::ALL
            .iter()
            .position(|mode| *mode == profile.ssl_mode())
            .unwrap_or_default();
        let ssl_mode =
            cx.new(|cx| SelectState::new(ssl_choices, Some(IndexPath::new(selected)), window, cx));
        let driver_choices: Vec<DriverChoice> = drivers
            .iter()
            .map(|driver| DriverChoice {
                id: driver.id(),
                title: driver.name().into(),
            })
            .collect();
        let driver_ix = driver_choices
            .iter()
            .position(|choice| choice.id == profile.driver())
            .unwrap_or_default();
        let driver_id = driver_choices
            .get(driver_ix)
            .map(|choice| choice.id)
            .unwrap_or(PostgresDriver::ID);
        let driver = cx.new(|cx| {
            SelectState::new(driver_choices, Some(IndexPath::new(driver_ix)), window, cx)
        });
        // A test result describes the fields as they were; any edit makes it
        // stale.
        let mut subscriptions: Vec<Subscription> = [
            &name,
            &file,
            &host,
            &port,
            &database,
            &user,
            &password,
            &ssh_host,
            &ssh_port,
            &ssh_user,
            &ssh_password,
            &ssh_key_file,
        ]
        .into_iter()
        .map(|input| {
            cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.forget_test(cx);
                }
            })
        })
        .collect();
        subscriptions.push(
            cx.subscribe(&ssl_mode, |this, _, _: &SelectEvent<Vec<SslChoice>>, cx| {
                this.forget_test(cx)
            }),
        );
        subscriptions.push(cx.subscribe_in(
            &driver,
            window,
            |this, _, event: &SelectEvent<Vec<DriverChoice>>, window, cx| {
                if let SelectEvent::Confirm(Some(id)) = event {
                    this.change_driver(id, window, cx);
                }
            },
        ));
        Self {
            existing,
            driver,
            driver_id,
            name,
            file,
            host,
            port,
            database,
            user,
            password,
            ssl_mode,
            use_ssh: profile.option(tunnel::HOST).is_some(),
            ssh_host,
            ssh_port,
            ssh_user,
            ssh_password,
            ssh_key_file,
            port_error: None,
            test: TestStatus::Idle,
            browse_task: None,
            _subscriptions: subscriptions,
        }
    }

    fn driver(&self, cx: &App) -> Option<Arc<dyn Driver>> {
        Services::global(cx).drivers().get(self.driver_id).cloned()
    }

    fn is_file_based(&self, cx: &App) -> bool {
        self.driver(cx).is_some_and(|driver| driver.is_file_based())
    }

    /// Switch the fields to `id`'s driver, replacing the port and user when
    /// they still hold the old driver's defaults.
    fn change_driver(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let drivers = Services::global(cx).drivers();
        let (Some(old), Some(new)) = (
            drivers.get(self.driver_id).cloned(),
            drivers.get(id).cloned(),
        ) else {
            return;
        };
        let port = self.port.read(cx).value().trim().to_string();
        if port.is_empty() || port == old.default_port().to_string() {
            let port = new.default_port().to_string();
            self.port
                .update(cx, |input, cx| input.set_value(port, window, cx));
        }
        let user = self.user.read(cx).value().trim().to_string();
        if user.is_empty() || user == old.default_user() {
            let user = new.default_user();
            self.user
                .update(cx, |input, cx| input.set_value(user, window, cx));
        }
        self.driver_id = id;
        self.forget_test(cx);
        cx.notify();
    }

    fn forget_test(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.test, TestStatus::Idle) {
            self.test = TestStatus::Idle;
            cx.notify();
        }
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        self.browse_task = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.set_file(path, cx);
            });
        }));
    }

    fn set_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let file = self.file.clone();
        let text = path.display().to_string();
        cx.defer(move |cx| {
            let Some(window) = cx.active_window() else {
                return;
            };
            let _ = window.update(cx, |_, window, cx| {
                file.update(cx, |input, cx| input.set_value(text, window, cx))
            });
        });
    }

    /// The profile the fields describe, or `None` when a field is invalid;
    /// the field then says why.
    fn profile(&mut self, cx: &App) -> Option<ConnectionProfile> {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let file_based = self.is_file_based(cx);
        let port = if file_based {
            0
        } else {
            match text(&self.port).parse::<u16>() {
                Ok(port) if port > 0 => port,
                _ => {
                    self.port_error = Some(t!("datasource.form.port_invalid").into());
                    return None;
                }
            }
        };
        self.port_error = None;
        let host = text(&self.host);
        let ssl_mode = self
            .ssl_mode
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or_default();
        let mut profile = ConnectionProfile::new(self.driver_id, port)
            .with_name(text(&self.name))
            .with_host(if host.is_empty() {
                "localhost".into()
            } else {
                host
            })
            .with_database(text(&self.database))
            .with_user(text(&self.user))
            .with_ssl_mode(ssl_mode);
        if file_based {
            profile = profile.with_option(ConnectionProfile::FILE, text(&self.file));
        } else if self.use_ssh {
            profile = profile
                .with_option(tunnel::HOST, text(&self.ssh_host))
                .with_option(tunnel::PORT, text(&self.ssh_port))
                .with_option(tunnel::USER, text(&self.ssh_user))
                .with_option(tunnel::KEY_FILE, text(&self.ssh_key_file));
        }
        if let Some(id) = &self.existing {
            profile = profile.with_id(id.clone());
        }
        Some(profile)
    }

    /// A password to store or test with: what was typed, or for an existing
    /// data source with nothing typed, `None` for the stored one.
    fn typed(&self, input: &Entity<InputState>, cx: &App) -> Option<String> {
        let typed = input.read(cx).value().to_string();
        if typed.is_empty() && self.existing.is_some() {
            None
        } else {
            Some(typed)
        }
    }

    fn test_connection(&mut self, cx: &mut Context<Self>) {
        let Some(profile) = self.profile(cx) else {
            cx.notify();
            return;
        };
        let password = self.typed(&self.password, cx);
        let ssh_password = self.typed(&self.ssh_password, cx);
        let task = open_connection(&profile, password, ssh_password, cx);
        self.test = TestStatus::Testing {
            _task: cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.test = match result {
                        Ok(connection) => TestStatus::Succeeded(
                            t!(
                                "datasource.form.test_succeeded",
                                server = connection.server_version()
                            )
                            .into(),
                        ),
                        Err(error) => TestStatus::Failed(describe_error(&error)),
                    };
                    cx.notify();
                });
            }),
        };
        cx.notify();
    }

    /// Add or update the data source. Returns whether the dialog may close.
    fn save(&mut self, _: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(profile) = self.profile(cx) else {
            cx.notify();
            return false;
        };
        // A new data source with no password stores nothing.
        let new = self.existing.is_none();
        let password = self
            .typed(&self.password, cx)
            .filter(|password| !(password.is_empty() && new));
        let ssh_password = self
            .typed(&self.ssh_password, cx)
            .filter(|password| !(password.is_empty() && new));
        DataSources::global(cx).update(cx, |data_sources, cx| {
            if new {
                data_sources.add(profile, password, ssh_password, cx);
            } else {
                data_sources.update(profile, password, ssh_password, cx);
            }
        });
        true
    }

    fn render_footer(&self, form: &Entity<Self>, cx: &App) -> impl IntoElement {
        let testing = matches!(self.test, TestStatus::Testing { .. });
        let status = match &self.test {
            TestStatus::Idle => None,
            TestStatus::Testing { .. } => Some(
                h_flex()
                    .gap_2()
                    .text_color(cx.theme().muted_foreground)
                    .child(Spinner::new().small())
                    .child(t!("datasource.form.testing").to_string())
                    .into_any_element(),
            ),
            TestStatus::Succeeded(message) => Some(
                div()
                    .text_color(cx.theme().success)
                    .child(message.clone())
                    .into_any_element(),
            ),
            TestStatus::Failed(message) => Some(
                div()
                    .text_color(cx.theme().danger)
                    .child(message.clone())
                    .into_any_element(),
            ),
        };
        DialogFooter::new()
            .justify_between()
            .items_start()
            .gap_4()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .child(
                        Button::new("test-connection")
                            .outline()
                            .icon(IconName::Plug)
                            .label(t!("datasource.form.test").to_string())
                            .loading(testing)
                            .on_click({
                                let form = form.clone();
                                move |_, _, cx| form.update(cx, |form, cx| form.test_connection(cx))
                            }),
                    )
                    .children(
                        status.map(|status| div().flex_1().min_w_0().text_sm().child(status)),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_none()
                    .child(
                        DialogClose::new().child(
                            Button::new("cancel")
                                .outline()
                                .label(t!("common.cancel").to_string()),
                        ),
                    )
                    .child(
                        DialogAction::new().child(
                            Button::new("save")
                                .primary()
                                .label(t!("common.save").to_string()),
                        ),
                    ),
            )
    }
}

fn ssl_mode_title(mode: SslMode) -> SharedString {
    match mode {
        SslMode::Disable => t!("datasource.ssl.disable"),
        SslMode::Prefer => t!("datasource.ssl.prefer"),
        SslMode::Require => t!("datasource.ssl.require"),
        SslMode::VerifyFull => t!("datasource.ssl.verify_full"),
    }
    .into()
}

impl Render for DataSourceForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let port_error = self.port_error.clone();
        let file_based = self.is_file_based(cx);
        let form = v_form()
            .columns(4)
            .child(
                field()
                    .col_span(4)
                    .label(t!("datasource.form.name").to_string())
                    .child(Input::new(&self.name)),
            )
            .child(
                field()
                    .col_span(4)
                    .label(t!("datasource.form.driver").to_string())
                    .child(Select::new(&self.driver)),
            );
        if file_based {
            return form.child(
                field()
                    .col_span(4)
                    .label(t!("datasource.form.file").to_string())
                    .child(
                        h_flex().gap_2().child(Input::new(&self.file)).child(
                            Button::new("browse")
                                .outline()
                                .label(t!("datasource.form.browse").to_string())
                                .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
                        ),
                    ),
            );
        }
        form.child(
            field()
                .col_span(3)
                .label(t!("datasource.form.host").to_string())
                .child(Input::new(&self.host)),
        )
        .child(
            field()
                .col_span(1)
                .label(t!("datasource.form.port").to_string())
                .child(Input::new(&self.port))
                .when_some(port_error, |field, error| {
                    let color = cx.theme().danger;
                    field.description_fn(move |_, _| div().text_color(color).child(error.clone()))
                }),
        )
        .child(
            field()
                .col_span(4)
                .label(t!("datasource.form.database").to_string())
                .child(Input::new(&self.database)),
        )
        .child(
            field()
                .col_span(2)
                .label(t!("datasource.form.user").to_string())
                .child(Input::new(&self.user)),
        )
        .child(
            field()
                .col_span(2)
                .label(t!("datasource.form.password").to_string())
                .child(Input::new(&self.password).mask_toggle()),
        )
        .child(
            field()
                .col_span(4)
                .label(t!("datasource.form.ssl_mode").to_string())
                .child(Select::new(&self.ssl_mode)),
        )
        .child(
            field().col_span(4).child(
                Checkbox::new("use-ssh")
                    .label(t!("datasource.form.use_ssh").to_string())
                    .checked(self.use_ssh)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.use_ssh = *checked;
                        this.forget_test(cx);
                        cx.notify();
                    })),
            ),
        )
        .when(self.use_ssh, |form| {
            form.child(
                field()
                    .col_span(3)
                    .label(t!("datasource.form.ssh_host").to_string())
                    .child(Input::new(&self.ssh_host)),
            )
            .child(
                field()
                    .col_span(1)
                    .label(t!("datasource.form.port").to_string())
                    .child(Input::new(&self.ssh_port)),
            )
            .child(
                field()
                    .col_span(2)
                    .label(t!("datasource.form.user").to_string())
                    .child(Input::new(&self.ssh_user)),
            )
            .child(
                field()
                    .col_span(2)
                    .label(t!("datasource.form.ssh_password").to_string())
                    .child(Input::new(&self.ssh_password).mask_toggle()),
            )
            .child(
                field()
                    .col_span(4)
                    .label(t!("datasource.form.ssh_key").to_string())
                    .child(Input::new(&self.ssh_key_file)),
            )
        })
    }
}
