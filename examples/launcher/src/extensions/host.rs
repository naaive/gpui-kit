use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui_kit::{App, Entity, SharedString, Window};
use gpui_shell::{ScriptView, ShellRuntime};

use super::{
    Extension, ExtensionCommand,
    bridge::{self, HostApi},
};
use crate::model::ToastStyle;

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

    /// Where toasts requested through `launcher/api` go.
    pub fn set_toast_handler(
        &self,
        handler: impl Fn(ToastStyle, SharedString, &mut App) + 'static,
    ) {
        self.api.set_toast_handler(handler);
    }

    /// Loads a command's module and mounts its View.
    ///
    /// The launch context is set first because a View reads it in `init`,
    /// which runs during mounting.
    pub fn open(
        &self,
        extension: &Extension,
        command: &ExtensionCommand,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Entity<ScriptView>> {
        self.api.set_launch(command.id().clone());
        let application = self
            .runtime
            .load_application(extension.root(), command.module())
            .with_context(|| format!("cannot load `{}`", command.id()))?;
        self.runtime
            .mount_application(&application, window, cx)
            .with_context(|| format!("cannot start `{}`", command.id()))
    }
}
