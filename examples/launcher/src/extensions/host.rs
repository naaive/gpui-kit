use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui_kit::{App, AppContext as _, Window};
use gpui_shell::ShellRuntime;

use super::{
    Extension, ExtensionCommand, LaunchRequest,
    bridge::{self, HostApi},
};
use crate::{
    model::Effect,
    pages::{self, PageHandle, ScriptPage},
};

/// What opening a command produced.
pub enum Opened {
    /// A page to push.
    Page(PageHandle),
    /// A no-view command that runs without a page.
    Background,
}

/// Runs extension commands on one shared GPUI Shell runtime.
///
/// Each extension will hold its own capability grant once permissions land;
/// until then every command runs under the runtime's default policy, which
/// grants nothing.
pub struct ExtensionHost {
    runtime: Rc<ShellRuntime>,
    api: Rc<HostApi>,
}

impl ExtensionHost {
    /// Creates the runtime. Call once, after `gpui_shell::init`.
    pub fn new(cx: &mut App) -> Result<Self> {
        let api = HostApi::export()?;
        let runtime = ShellRuntime::new_with_components(cx, bridge::components()?)?;
        Ok(Self { runtime, api })
    }

    /// Where effects requested through `launcher/api` go.
    pub fn set_effect_handler(&self, handler: impl Fn(Effect, &mut App) + 'static) {
        self.api.set_effect_handler(handler);
    }

    /// Loads a command's module, mounts its View and wraps it as a page.
    ///
    /// The launch context is set first because a View reads it in `init`,
    /// which runs during mounting.
    pub fn open(
        &self,
        extension: &Extension,
        command: &ExtensionCommand,
        request: &LaunchRequest,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Opened> {
        let _ = request;
        self.api.set_launch(command.id().clone());
        let application = self
            .runtime
            .load_application(extension.root(), command.module())
            .with_context(|| format!("cannot load `{}`", command.id()))?;
        let view = self
            .runtime
            .mount_application(&application, window, cx)
            .with_context(|| format!("cannot start `{}`", command.id()))?;
        let title = command.title().clone();
        let page = cx.new(|cx| ScriptPage::new(title, view, cx));
        Ok(Opened::Page(pages::handle(page)))
    }
}

/// Builds the page an extension pushes: `callback` returns an instance of a
/// `View` subclass, which becomes a page of its own.
pub fn page_from_callback(
    _callback: &gpui_shell::ComponentCallback,
    _title: gpui_kit::SharedString,
    _window: &mut Window,
    _cx: &mut App,
) -> Result<PageHandle> {
    anyhow::bail!("pushing an extension page is not supported by this build")
}
