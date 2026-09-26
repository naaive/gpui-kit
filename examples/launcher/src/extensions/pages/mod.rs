//! The launcher's own pages about extensions: the questions asked before a
//! command runs (permissions, preferences, arguments) and the extension
//! manager.
//!
//! They are ordinary [`Page`](crate::pages::Page)s producing models, drawn by
//! the same renderer as everything else. A question page answers by asking
//! the window to pop it and launch the original request again, so the host
//! re-checks everything in one place rather than each page continuing on its
//! own.

mod arguments;
mod extensions;
mod permission;
mod preferences;

pub use arguments::ArgumentsPage;
pub use extensions::ExtensionsPage;
pub use permission::PermissionPage;
pub use preferences::PreferencesPage;

use gpui_kit::{App, SharedString};

use super::{LaunchRequest, host::Services};
use crate::model::{Effect, FormValue, FormValues, Toast, ToastStyle};

/// Pops the question page and opens the command again.
fn continue_launch(services: &Services, request: &LaunchRequest, cx: &mut App) {
    services.effects.request(Effect::Pop, cx);
    services
        .effects
        .request(Effect::Launch(request.clone()), cx);
}

fn show_failure(services: &Services, title: &str, error: &anyhow::Error, cx: &mut App) {
    services.effects.toast(
        Toast::new(ToastStyle::Failure, title.to_owned()).with_message(format!("{error:#}")),
        cx,
    );
}

/// The text a field was submitted with; `None` when the form did not report
/// the field at all.
fn submitted_text(values: &FormValues, id: &str) -> Option<SharedString> {
    match values.get(id)? {
        FormValue::Text(text) => Some(text.clone()),
        FormValue::Empty => Some(SharedString::default()),
        FormValue::Bool(value) => Some(value.to_string().into()),
    }
}

/// The error shown under a required field left blank.
const REQUIRED: &str = "Required";
