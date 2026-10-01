//! Now Playing: every application playing media, the current one first,
//! refreshed every couple of seconds while the page is open.

use std::time::Duration;

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, WeakEntity, Window};

use super::{
    platform,
    session::{Control, Session, Status},
};
use crate::{
    model::{
        Accessory, Action, ActionPanel, DetailModel, Effect, Image, Item, ItemId, ListModel,
        Metadata, MetadataValue, PageModel, RunHandler, Section, Toast, ToastStyle, Tone,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
};

const REFRESH: Duration = Duration::from_secs(2);
/// How long an application takes to show the result of a command.
pub const SETTLE: Duration = Duration::from_millis(400);

pub fn now_playing_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|cx: &mut Context<NowPlayingPage>| {
        let task = cx.spawn(async move |this, cx| {
            loop {
                let sessions = cx
                    .background_spawn(async { platform::sessions().map_err(|e| format!("{e:#}")) })
                    .await;
                let alive = this
                    .update(cx, |page: &mut NowPlayingPage, cx| {
                        page.sessions = Some(sessions);
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        });
        NowPlayingPage {
            sessions: None,
            _refresh: task,
            control: None,
        }
    })))
}

pub struct NowPlayingPage {
    /// `None` until the first reading.
    sessions: Option<Result<Vec<Session>, String>>,
    _refresh: Task<()>,
    /// The command in flight, then the reading after it.
    control: Option<Task<()>>,
}

impl NowPlayingPage {
    fn send(&mut self, control: Control, app_id: String, ix: usize, cx: &mut Context<Self>) {
        self.control = Some(cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let result = cx
                .background_spawn(async move {
                    platform::send(control, Some((&app_id, ix)))?;
                    executor.timer(SETTLE).await;
                    platform::sessions()
                })
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(sessions) => page.sessions = Some(Ok(sessions)),
                    Err(error) => perform(
                        Effect::ShowToast(
                            Toast::new(ToastStyle::Failure, control.failure())
                                .with_message(format!("{error:#}")),
                        ),
                        cx,
                    ),
                }
                page.control = None;
                cx.notify();
            })
            .ok();
        }));
    }
}

fn control_action(
    page: &WeakEntity<NowPlayingPage>,
    session: &Session,
    control: Control,
) -> Action {
    let page = page.clone();
    let app_id = session.app_id.clone();
    let ix = session.ix;
    let (title, icon) = match control {
        Control::PlayPause => (
            match session.status {
                Status::Playing => "Pause",
                _ => "Play",
            },
            match session.status {
                Status::Playing => "pause",
                _ => "play",
            },
        ),
        Control::Next => ("Next Track", "skip-forward"),
        Control::Previous => ("Previous Track", "skip-back"),
    };
    Action::new(
        title,
        Effect::Run(RunHandler::new(move |(), _, cx| {
            let app_id = app_id.clone();
            page.update(cx, |page, cx| page.send(control, app_id, ix, cx))
                .ok();
        })),
    )
    .with_image(Image::Icon(icon.into()))
}

fn session_item(session: &Session, page: &WeakEntity<NowPlayingPage>) -> Item {
    let image = match &session.artwork {
        Some(path) => Image::File(path.clone()),
        None => Image::Icon("music".into()),
    };
    let tone = match session.status {
        Status::Playing => Tone::Success,
        _ => Tone::Neutral,
    };
    let mut detail = DetailModel::new(format!("## {}", session.display_title()));
    if let Some(path) = &session.artwork {
        detail = detail.with_image(path.clone());
    }
    let text = |text: &str| MetadataValue::Text(text.to_owned().into());
    for (label, value) in [("Artist", &session.artist), ("Album", &session.album)] {
        if !value.trim().is_empty() {
            detail = detail.with_metadata(Metadata::new(label, text(value)));
        }
    }
    detail = detail
        .with_metadata(Metadata::new("Application", text(&session.app_name())))
        .with_metadata(Metadata::new(
            "Status",
            MetadataValue::Tags(vec![
                crate::model::Tag::new(session.status.label()).with_tone(tone),
            ]),
        ));
    if let Some(progress) = session.progress() {
        detail = detail.with_metadata(Metadata::new("Position", text(&progress)));
    }
    let mut item = Item::new(ItemId::new(session.key()), session.display_title())
        .with_image(image)
        .with_keyword(session.app_name())
        .with_keyword(session.album.clone())
        .with_accessory(Accessory::tag(session.status.label(), tone))
        .with_detail(detail)
        .with_actions(
            ActionPanel::new()
                .with_action(control_action(page, session, Control::PlayPause))
                .with_action(control_action(page, session, Control::Next))
                .with_action(control_action(page, session, Control::Previous))
                .with_action(Action::new(
                    "Copy Title",
                    Effect::Copy(session.hud_text().into()),
                )),
        );
    let byline = session.byline();
    if !byline.is_empty() {
        item = item.with_subtitle(byline);
    }
    item
}

impl Page for NowPlayingPage {
    fn title(&self) -> SharedString {
        "Now Playing".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let list = ListModel::new()
            .with_placeholder("Search media…")
            .with_loading(self.sessions.is_none() || self.control.is_some())
            .with_showing_detail(true);
        let sessions = match &self.sessions {
            None => return list.with_empty_title("Reading media…").into(),
            Some(Err(error)) => {
                return PageModel::failure("Couldn’t read the media controls", error.clone());
            }
            Some(Ok(sessions)) => sessions,
        };
        let list = list
            .with_empty_title("Nothing is playing")
            .with_empty_description(
                "Media playing in an app that shows in the system’s media controls appears here.",
            );
        let page = cx.entity().downgrade();
        let (current, others): (Vec<&Session>, Vec<&Session>) =
            sessions.iter().partition(|session| session.is_current);
        list.with_section(
            Section::new()
                .with_title("Now Playing")
                .with_items(current.iter().map(|session| session_item(session, &page))),
        )
        .with_section(
            Section::new()
                .with_title("Other Apps")
                .with_items(others.iter().map(|session| session_item(session, &page))),
        )
        .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
