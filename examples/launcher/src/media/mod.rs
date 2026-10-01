//! Media control through the system's media controls: what each application
//! is playing, and play/pause and skipping without leaving the keyboard.
//!
//! Windows only, through `Windows.Media.Control`; elsewhere no commands are
//! offered.

#[cfg(target_os = "windows")]
mod page;
#[cfg(target_os = "windows")]
mod platform;
#[cfg(any(target_os = "windows", test))]
mod session;

use crate::model::Item;

/// The root search commands this module offers on this platform.
#[cfg(target_os = "windows")]
pub fn commands() -> Vec<Item> {
    use crate::{
        model::{Action, Effect, PushHandler},
        sources::system::command_item,
    };
    use session::Control;

    let control = |id, title, icon, control: Control| {
        command_item(id, title, icon)
            .with_keyword("media")
            .with_keyword("music")
            .with_action(Action::new(title, Effect::Run(control_current(control))))
    };
    vec![
        command_item("system/now-playing", "Now Playing", "music")
            .with_keyword("media")
            .with_keyword("music")
            .with_keyword("song")
            .with_keyword("track")
            .with_action(Action::new(
                "Now Playing",
                Effect::Push(PushHandler::new(page::now_playing_page)),
            )),
        control(
            "system/play-pause",
            "Play/Pause Media",
            "circle-play",
            Control::PlayPause,
        )
        .with_keyword("pause")
        .with_keyword("resume"),
        control(
            "system/next-track",
            "Next Track",
            "skip-forward",
            Control::Next,
        )
        .with_keyword("skip"),
        control(
            "system/previous-track",
            "Previous Track",
            "skip-back",
            Control::Previous,
        )
        .with_keyword("back"),
    ]
}

#[cfg(not(target_os = "windows"))]
pub fn commands() -> Vec<Item> {
    Vec::new()
}

/// Sends `control` to the current session in the background, then shows
/// the track it leaves playing in a HUD, or a toast saying why it failed.
#[cfg(target_os = "windows")]
fn control_current(control: session::Control) -> crate::model::RunHandler {
    use gpui_kit::AppContext as _;

    use crate::{
        model::{Effect, RunHandler, Toast, ToastStyle},
        shell::launcher::perform,
    };

    RunHandler::new(move |(), _, cx| {
        let executor = cx.background_executor().clone();
        let task = cx.background_spawn(async move {
            platform::send(control, None)?;
            executor.timer(page::SETTLE).await;
            platform::current()
        });
        cx.spawn(async move |cx| {
            let effect = match task.await {
                Ok(session) => Effect::ShowHud(
                    session
                        .map(|session| session.hud_text())
                        .unwrap_or_else(|| control.title().to_owned())
                        .into(),
                ),
                Err(error) => Effect::ShowToast(
                    Toast::new(ToastStyle::Failure, control.failure())
                        .with_message(format!("{error:#}")),
                ),
            };
            cx.update(|cx| perform(effect, cx));
        })
        .detach();
    })
}
