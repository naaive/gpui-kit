//! `launcher/api`: host functions for extensions.
//!
//! Host functions exchange plain data only and never hold a script value
//! (GPUI Shell §17.6), so everything here is a question the host answers or a
//! request it carries out after the call returns.

use std::{cell::RefCell, rc::Rc};

use anyhow::{Result, anyhow};
use gpui_kit::App;
use gpui_shell::{HostError, HostModule, HostObject, HostValue};

use crate::{
    extensions::CommandId,
    model::{Effect, Toast, ToastStyle},
};

pub const MODULE: &str = "launcher/api";

const DECLARATIONS: &str = r#"
/** The command this page was opened for. Read it in `init`. */
export function launch(): { extension: string; command: string };
/** Shows a message in the launcher. */
export function show_toast(message: string, style?: "info" | "success" | "failure"): void;
"#;

type EffectHandler = Rc<dyn Fn(Effect, &mut App)>;

/// State shared between the launcher and the functions it exports.
#[derive(Default)]
pub struct HostApi {
    launch: RefCell<Option<CommandId>>,
    on_effect: RefCell<Option<EffectHandler>>,
}

impl HostApi {
    /// Registers `launcher/api` for every runtime on this thread.
    pub fn export() -> Result<Rc<Self>> {
        let api = Rc::new(Self::default());
        let launch = api.clone();
        let toast = api.clone();
        let module = HostModule::new(MODULE)
            .function("launch", move |_| launch.launch_value())
            .function("show_toast", move |arguments| {
                let message = arguments.string(0)?.to_owned();
                let style = match arguments.get(1) {
                    None | Some(HostValue::Null) => ToastStyle::Info,
                    Some(_) => match arguments.string(1)? {
                        "info" => ToastStyle::Info,
                        "success" => ToastStyle::Success,
                        "failure" => ToastStyle::Failure,
                        other => {
                            return Err(HostError::new(format!(
                                "unknown toast style `{other}`; use info, success or failure"
                            )));
                        }
                    },
                };
                toast.request(Effect::ShowToast(Toast::new(style, message)));
                Ok(HostValue::Null)
            })
            .declarations(DECLARATIONS);
        gpui_shell::export_module(module).map_err(|error| anyhow!("{error}"))?;
        Ok(api)
    }

    /// Records which command the next mounted view belongs to.
    pub fn set_launch(&self, command: CommandId) {
        self.launch.replace(Some(command));
    }

    /// Where requested effects go; the launcher window performs them.
    pub fn set_effect_handler(&self, handler: impl Fn(Effect, &mut App) + 'static) {
        self.on_effect.replace(Some(Rc::new(handler)));
    }

    fn launch_value(&self) -> Result<HostValue, HostError> {
        let launch = self.launch.borrow();
        let command = launch
            .as_ref()
            .ok_or_else(|| HostError::new("launch() is only available to a command's view"))?;
        Ok(HostObject::new()
            .field("extension", command.extension().to_string())
            .field("command", command.command().to_string())
            .into())
    }

    fn request(&self, effect: Effect) {
        let Some(handler) = self.on_effect.borrow().clone() else {
            tracing::info!("effect requested with no launcher window: {effect:?}");
            return;
        };
        gpui_shell::with_current_app(|cx| handler(effect, cx));
    }
}
