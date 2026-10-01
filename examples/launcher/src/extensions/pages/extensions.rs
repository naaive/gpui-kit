use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, WeakEntity, Window};

use super::{PreferencesPage, REQUIRED, show_failure, submitted_text};
use crate::{
    extensions::{
        Catalog,
        host::Services,
        install::{self, InstalledExtension},
        permissions::{PermissionState, RequestedCapabilities},
        preferences::all_scopes,
    },
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation,
        Control, Effect, Field, FormHandler, FormModel, FormValues, Item, ItemId, ListModel,
        PageModel, PushHandler, RunHandler, Section, Toast, ToastStyle,
    },
    pages::{self, Page, PageHandle},
};

/// The extensions installed from Git, and installing more.
///
/// Only the launcher's own extension directory is managed here: bundled
/// extensions and a developer's `LAUNCHER_EXTENSIONS` directory are not the
/// launcher's to update or remove.
pub struct ExtensionsPage {
    services: Services,
    extensions: Vec<InstalledExtension>,
}

impl ExtensionsPage {
    pub fn new(services: Services) -> Result<Self> {
        let extensions = install::installed(&services.data)?;
        Ok(Self {
            services,
            extensions,
        })
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        match install::installed(&self.services.data) {
            Ok(extensions) => self.extensions = extensions,
            Err(error) => show_failure(&self.services, "Couldn’t list extensions", &error, cx),
        }
        cx.notify();
    }

    fn item(
        &self,
        extension: &InstalledExtension,
        page: &WeakEntity<Self>,
        catalog: &Catalog,
    ) -> Item {
        let id = extension.id().to_owned();
        let name = extension.name().to_owned();
        let has_preferences = all_scopes(catalog, &id)
            .iter()
            .any(|(_, declarations)| !declarations.is_empty());

        let mut actions = ActionPanel::new();
        if has_preferences {
            let (services, id, name) = (self.services.clone(), id.clone(), name.clone());
            actions = actions.with_action(Action::new(
                "Open Preferences…",
                Effect::Push(PushHandler::new(move |_, cx| {
                    let catalog = Catalog::discover(&[services.data.extensions_dir()]);
                    let page = PreferencesPage::new(
                        format!("{name} Preferences"),
                        all_scopes(&catalog, &id),
                        services.clone(),
                        None,
                    );
                    Ok(pages::handle(cx.new(|_| page)))
                })),
            ));
        }
        if extension.source().is_some() {
            actions = actions.with_action(Action::new(
                "Update",
                Effect::Run(update_handler(
                    self.services.clone(),
                    id.clone(),
                    name.clone(),
                    page.clone(),
                )),
            ));
        }
        actions = actions
            .with_action(Action::new(
                "Reveal in File Manager",
                Effect::RevealPath(extension.directory().to_path_buf()),
            ))
            .with_section(
                ActionSection::new().with_entry(ActionEntry::Action(
                    Action::new(
                        "Uninstall",
                        Effect::Confirm(
                            Confirmation::new(
                                format!("Uninstall “{name}”?"),
                                Effect::Run(uninstall_handler(
                                    self.services.clone(),
                                    id.clone(),
                                    page.clone(),
                                )),
                            )
                            .with_message(
                                "Its preferences, permissions and stored data are removed too.",
                            )
                            .with_confirm_title("Uninstall")
                            .destructive(true),
                        ),
                    )
                    .with_style(ActionStyle::Destructive),
                )),
            );

        let subtitle = extension
            .source()
            .map(ToString::to_string)
            .unwrap_or_else(|| id.clone());
        Item::new(ItemId::new(format!("extension/{id}")), name)
            .with_subtitle(subtitle)
            .with_icon("package")
            .with_accessory(Accessory::text(extension.version().to_owned()))
            .with_keyword(id)
            .with_actions(actions)
    }
}

impl Page for ExtensionsPage {
    fn title(&self) -> SharedString {
        "Extensions".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let catalog = Catalog::discover(&[self.services.data.extensions_dir()]);
        let services = self.services.clone();
        let install = Item::new(ItemId::new("install"), "Install from Git…")
            .with_icon("download")
            .with_action(Action::new(
                "Install from Git…",
                Effect::Push(PushHandler::new({
                    let page = page.clone();
                    move |_, cx| install_page(services.clone(), page.clone(), cx)
                })),
            ));
        let installed = Section::new().with_title("Installed").with_items(
            self.extensions
                .iter()
                .map(|extension| self.item(extension, &page, &catalog)),
        );
        PageModel::List(
            ListModel::new()
                .with_placeholder("Search extensions…")
                .with_item(install)
                .with_section(installed)
                .with_empty_title("No extensions installed"),
        )
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

/// Runs `work` off the main thread with a progress toast that turns into the
/// outcome, then hands the result back on the main thread.
fn in_background<T: Send + 'static>(
    services: &Services,
    progress: String,
    toast_id: String,
    work: impl FnOnce() -> Result<T> + Send + 'static,
    done: impl FnOnce(Result<T>, &mut App) -> Toast + 'static,
    cx: &mut App,
) {
    services.effects.toast(
        Toast::new(ToastStyle::Progress, progress).with_id(toast_id.clone()),
        cx,
    );
    let task = cx.background_executor().spawn(async move { work() });
    let services = services.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            let toast = done(result, cx).with_id(toast_id);
            services.effects.toast(toast, cx);
        });
    })
    .detach();
}

/// Reloads the extensions page, if it is still open, and lets the window
/// read the catalog again.
fn refresh(services: &Services, page: &WeakEntity<ExtensionsPage>, cx: &mut App) {
    page.update(cx, |page, cx| page.reload(cx)).ok();
    services.changed.notify(cx);
}

fn update_handler(
    services: Services,
    id: String,
    name: String,
    page: WeakEntity<ExtensionsPage>,
) -> RunHandler {
    RunHandler::new(move |(), _, cx| {
        let data = services.data.clone();
        let (services, page, work_id, name) =
            (services.clone(), page.clone(), id.clone(), name.clone());
        in_background(
            &services.clone(),
            format!("Updating “{name}”…"),
            format!("update/{id}"),
            move || install::update(&data, &work_id),
            move |result, cx| match result {
                Ok(updated) => {
                    refresh(&services, &page, cx);
                    // Growing its request grants nothing: the next run asks.
                    let asks = RequestedCapabilities::read(updated.directory())
                        .and_then(|requested| services.permissions.state(updated.id(), &requested))
                        .is_ok_and(|state| matches!(state, PermissionState::Ask(_)));
                    let toast = Toast::new(
                        ToastStyle::Success,
                        format!("“{}” is at {}", updated.name(), updated.version()),
                    );
                    if asks {
                        toast.with_message("It asks for new permissions the next time it runs.")
                    } else {
                        toast
                    }
                }
                Err(error) => Toast::new(ToastStyle::Failure, format!("Couldn’t update “{name}”"))
                    .with_message(format!("{error:#}")),
            },
            cx,
        );
    })
}

fn uninstall_handler(
    services: Services,
    id: String,
    page: WeakEntity<ExtensionsPage>,
) -> RunHandler {
    RunHandler::new(move |(), _, cx| {
        match install::uninstall(
            &services.data,
            &id,
            &services.permissions,
            &services.preferences,
        ) {
            Ok(()) => refresh(&services, &page, cx),
            Err(error) => show_failure(&services, "Couldn’t uninstall the extension", &error, cx),
        }
    })
}

fn install_page(
    services: Services,
    extensions: WeakEntity<ExtensionsPage>,
    cx: &mut App,
) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|_| InstallPage {
        services,
        extensions,
        error: None,
    })))
}

const REPOSITORY: &str = "repository";

/// Asks for the repository to install from.
struct InstallPage {
    services: Services,
    extensions: WeakEntity<ExtensionsPage>,
    error: Option<SharedString>,
}

impl InstallPage {
    fn submit(&mut self, values: FormValues, cx: &mut Context<Self>) {
        let source = submitted_text(&values, REPOSITORY).unwrap_or_default();
        let source = source.trim().to_owned();
        if source.is_empty() {
            self.error = Some(REQUIRED.into());
            cx.notify();
            return;
        }
        if let Err(error) = install::GitSource::parse(&source) {
            self.error = Some(format!("{error:#}").into());
            cx.notify();
            return;
        }
        self.services.effects.request(Effect::Pop, cx);
        let data = self.services.data.clone();
        let (services, page) = (self.services.clone(), self.extensions.clone());
        let work_source = source.clone();
        in_background(
            &self.services,
            format!("Installing {source}…"),
            format!("install/{source}"),
            move || install::install(&data, &work_source),
            move |result, cx| match result {
                Ok(installed) => {
                    refresh(&services, &page, cx);
                    Toast::new(
                        ToastStyle::Success,
                        format!("Installed “{}” {}", installed.name(), installed.version()),
                    )
                }
                Err(error) => Toast::new(ToastStyle::Failure, "Couldn’t install the extension")
                    .with_message(format!("{error:#}")),
            },
            cx,
        );
    }
}

impl Page for InstallPage {
    fn title(&self) -> SharedString {
        "Install from Git".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let field = Field::new(
            REPOSITORY,
            "Repository",
            Control::Text {
                placeholder: Some("owner/repo or a Git URL".into()),
                value: SharedString::default(),
            },
        )
        .with_info("Add #branch, #tag or #commit to follow a ref other than the default branch.");
        let field = match &self.error {
            Some(error) => field.with_error(error.clone()),
            None => field,
        };
        PageModel::Form(FormModel::new().with_field(field).with_actions(
            ActionPanel::new().with_action(Action::new(
                "Install",
                Effect::SubmitForm(FormHandler::new(move |values, _, cx| {
                    page.update(cx, |page, cx| page.submit(values, cx)).ok();
                })),
            )),
        ))
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
