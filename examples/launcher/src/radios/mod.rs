//! Wi-Fi and Bluetooth: turning the radios on and off, and the network the
//! computer is connected to.
//!
//! Windows only, through `Windows.Devices.Radios`; elsewhere no commands are
//! offered.

#[cfg(any(target_os = "windows", test))]
mod netsh;
#[cfg(target_os = "windows")]
mod page;
#[cfg(target_os = "windows")]
mod platform;
#[cfg(any(target_os = "windows", test))]
mod radio;

use crate::model::Item;

/// The root search commands this module offers on this platform.
#[cfg(target_os = "windows")]
pub fn commands() -> Vec<Item> {
    use radio::Technology;

    use crate::{
        model::{Action, Effect, PushHandler},
        sources::system::command_item,
    };

    vec![
        command_item("system/toggle-wifi", "Toggle Wi-Fi", "wifi")
            .with_keyword("wireless")
            .with_keyword("network")
            .with_keyword("wlan")
            .with_action(Action::new(
                "Toggle Wi-Fi",
                Effect::Run(toggle(Technology::WiFi)),
            )),
        command_item("system/toggle-bluetooth", "Toggle Bluetooth", "bluetooth")
            .with_keyword("wireless")
            .with_action(Action::new(
                "Toggle Bluetooth",
                Effect::Run(toggle(Technology::Bluetooth)),
            )),
        command_item("system/network-status", "Network Status", "network")
            .with_keyword("wi-fi")
            .with_keyword("wifi")
            .with_keyword("bluetooth")
            .with_keyword("ssid")
            .with_keyword("radio")
            .with_action(Action::new(
                "Network Status",
                Effect::Push(PushHandler::new(page::network_status_page)),
            )),
    ]
}

#[cfg(not(target_os = "windows"))]
pub fn commands() -> Vec<Item> {
    Vec::new()
}

/// Toggles the radios of `technology` in the background, then says the new
/// state in a HUD, or why it failed in a toast.
#[cfg(target_os = "windows")]
fn toggle(technology: radio::Technology) -> crate::model::RunHandler {
    use gpui_kit::AppContext as _;

    use crate::{
        model::{Effect, RunHandler, Toast, ToastStyle},
        shell::launcher::perform,
    };

    RunHandler::new(move |(), _, cx| {
        let task = cx.background_spawn(async move { platform::toggle(technology) });
        cx.spawn(async move |cx| {
            let effect = match task.await {
                Ok(power) => Effect::ShowHud(radio::hud_text(technology, power).into()),
                Err(error) => Effect::ShowToast(
                    Toast::new(
                        ToastStyle::Failure,
                        format!("Couldn’t change {}", technology.label()),
                    )
                    .with_message(error.to_string()),
                ),
            };
            cx.update(|cx| perform(effect, cx));
        })
        .detach();
    })
}
