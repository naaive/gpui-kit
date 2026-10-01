//! Start Timer, where the search text is the timer's length and name, and
//! Running Timers, counting down every second.

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Subscription, Task, Window};

use super::{
    TICK, Timer, Timers,
    duration::{default_name, format_clock, format_length, parse_query},
    format_time, now, store, update,
};
use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Effect, Image,
        Item, ItemId, ListModel, PageModel, PushHandler, RunHandler, Section, Tone,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
};

/// The lengths offered before anything is typed, in minutes.
const PRESETS: [u64; 9] = [1, 3, 5, 10, 15, 25, 30, 45, 60];

const MINUTE: i64 = 60_000;

/// A task that redraws the page every second while it is open.
fn redraw_every_second<P: Page>(cx: &mut Context<P>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(TICK).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        }
    })
}

pub fn start_timer_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let store = store(cx);
    Ok(pages::handle(cx.new(|cx| StartTimerPage {
        _subscription: cx.observe(&store, |_, _, cx| cx.notify()),
        _tick: redraw_every_second(cx),
        store,
        query: String::new(),
    })))
}

struct StartTimerPage {
    store: Entity<Timers>,
    query: String,
    _subscription: Subscription,
    _tick: Task<()>,
}

/// Starts a timer, then hides the launcher saying when it ends.
fn start(seconds: u64, name: String) -> Effect {
    Effect::Run(RunHandler::new(move |(), _, cx| {
        let timer = store(cx).update(cx, |timers, cx| {
            timers.start_timer(seconds, name.clone(), cx)
        });
        let ends = timer
            .ends_at()
            .map(|at| format_time(at, now()))
            .unwrap_or_default();
        perform(
            Effect::ShowHud(format!("{} started · ends at {ends}", timer.title()).into()),
            cx,
        );
    }))
}

fn start_item(id: String, seconds: u64, name: &str, now: i64) -> Item {
    let ends = format_time(now + seconds as i64 * 1000, now);
    let mut item = Item::new(ItemId::new(id), format_length(seconds))
        .with_image(Image::Icon("timer".into()))
        .with_accessory(Accessory::text(format!("Ends at {ends}")))
        .with_action(Action::new("Start Timer", start(seconds, name.to_owned())));
    if !name.is_empty() {
        item = item.with_subtitle(name.to_owned());
    }
    item
}

impl Page for StartTimerPage {
    fn title(&self) -> SharedString {
        "Start Timer".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let now = now();
        let query = self.query.trim();
        let parsed = parse_query(query);
        // Text that is not a length names the preset picked.
        let name = match &parsed {
            Some(_) => String::new(),
            None => query.to_owned(),
        };
        let mut list = ListModel::new()
            .with_filtering(false)
            .with_placeholder("Length and name, such as 25m tea…");
        if let Some((seconds, typed_name)) = &parsed {
            let ends = format_time(now + *seconds as i64 * 1000, now);
            let title = match typed_name.is_empty() {
                true => format!("Start {}", default_name(*seconds)),
                false => format!("Start “{typed_name}”"),
            };
            list = list.with_item(
                Item::new(ItemId::new("timer/start"), title)
                    .with_image(Image::Icon("timer".into()))
                    .with_subtitle(format_length(*seconds))
                    .with_accessory(Accessory::text(format!("Ends at {ends}")))
                    .with_action(Action::new(
                        "Start Timer",
                        start(*seconds, typed_name.clone()),
                    )),
            );
        }
        let recent: Vec<Item> = self
            .store
            .read(cx)
            .recent()
            .iter()
            .map(|&seconds| start_item(format!("timer/recent/{seconds}"), seconds, &name, now))
            .collect();
        let presets = PRESETS.iter().map(|&minutes| {
            start_item(format!("timer/preset/{minutes}"), minutes * 60, &name, now)
        });
        list.with_section(Section::new().with_title("Recent").with_items(recent))
            .with_section(Section::new().with_title("Presets").with_items(presets))
            .into()
    }

    fn set_query(&mut self, query: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query.to_owned();
            cx.notify();
        }
    }
}

pub fn running_timers_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let store = store(cx);
    Ok(pages::handle(cx.new(|cx| RunningTimersPage {
        _subscription: cx.observe(&store, |_, _, cx| cx.notify()),
        _tick: redraw_every_second(cx),
        store,
    })))
}

struct RunningTimersPage {
    store: Entity<Timers>,
    _subscription: Subscription,
    _tick: Task<()>,
}

fn change(id: &str, change: fn(&mut Timer, i64)) -> Effect {
    let id = id.to_owned();
    Effect::Run(RunHandler::new(move |(), _, cx| {
        update(cx, |timers, cx| timers.change(&id, change, cx))
    }))
}

fn timer_item(timer: &Timer, now: i64) -> Item {
    let ended = timer.is_ended(now);
    let toggle = match (ended, timer.is_paused()) {
        (true, _) => None,
        (false, true) => Some(
            Action::new("Resume", change(&timer.id, Timer::resume))
                .with_image(Image::Icon("play".into())),
        ),
        (false, false) => Some(
            Action::new("Pause", change(&timer.id, Timer::pause))
                .with_image(Image::Icon("pause".into())),
        ),
    };
    let stop = timer.id.clone();
    let first = match toggle {
        Some(toggle) => ActionSection::new().with_entry(ActionEntry::Action(toggle)),
        None => ActionSection::new(),
    };
    let actions = ActionPanel::new()
        .with_section(
            first
                .with_entry(ActionEntry::Action(
                    Action::new("Restart", change(&timer.id, Timer::restart))
                        .with_image(Image::Icon("rotate-ccw".into()))
                        .with_shortcut("secondary-r"),
                ))
                .with_entry(ActionEntry::Action(
                    Action::new(
                        "Add 1 Minute",
                        change(&timer.id, |timer, now| timer.extend(MINUTE, now)),
                    )
                    .with_image(Image::Icon("plus".into()))
                    .with_shortcut("secondary-1"),
                ))
                .with_entry(ActionEntry::Action(
                    Action::new(
                        "Add 5 Minutes",
                        change(&timer.id, |timer, now| timer.extend(5 * MINUTE, now)),
                    )
                    .with_image(Image::Icon("plus".into()))
                    .with_shortcut("secondary-5"),
                ))
                .with_entry(ActionEntry::Action(
                    Action::new(
                        "Start Timer",
                        Effect::Push(PushHandler::new(start_timer_page)),
                    )
                    .with_image(Image::Icon("timer".into()))
                    .with_shortcut("secondary-n"),
                )),
        )
        .with_section(
            ActionSection::new().with_entry(ActionEntry::Action(
                Action::new(
                    "Stop Timer",
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        update(cx, |timers, cx| timers.remove(&stop, cx))
                    })),
                )
                .with_image(Image::Icon("circle-stop".into()))
                .with_style(ActionStyle::Destructive)
                .with_shortcut("ctrl-x"),
            )),
        );
    let (icon, state) = match (ended, timer.is_paused()) {
        (true, _) => (
            "alarm-clock-check",
            Accessory::tag(
                format!(
                    "Done at {}",
                    timer
                        .ends_at()
                        .map(|at| format_time(at, now))
                        .unwrap_or_default()
                ),
                Tone::Success,
            ),
        ),
        (false, true) => ("pause", Accessory::tag("Paused", Tone::Neutral)),
        (false, false) => (
            "timer",
            Accessory::text(format!(
                "Ends at {}",
                timer
                    .ends_at()
                    .map(|at| format_time(at, now))
                    .unwrap_or_default()
            )),
        ),
    };
    let mut item = Item::new(ItemId::new(format!("timer/{}", timer.id)), timer.title())
        .with_image(Image::Icon(icon.into()))
        .with_subtitle(format_length((timer.length / 1000) as u64))
        .with_accessory(state)
        .with_actions(actions);
    if !ended {
        item = item.with_accessory(Accessory::text(format_clock(timer.remaining_seconds(now))));
    }
    item
}

impl Page for RunningTimersPage {
    fn title(&self) -> SharedString {
        "Running Timers".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let now = now();
        let mut timers: Vec<&Timer> = self.store.read(cx).timers().iter().collect();
        // Ended first, then the soonest to end; paused ones last.
        timers.sort_by_key(|timer| {
            (
                !timer.is_ended(now),
                timer.is_paused(),
                timer.remaining(now),
            )
        });
        let start = timers.is_empty().then(|| {
            Item::new(ItemId::new("timer/new"), "Start Timer")
                .with_image(Image::Icon("timer".into()))
                .with_action(Action::new(
                    "Start Timer",
                    Effect::Push(PushHandler::new(start_timer_page)),
                ))
        });
        ListModel::new()
            .with_placeholder("Search timers…")
            .with_section(
                Section::new()
                    .with_items(timers.into_iter().map(|timer| timer_item(timer, now)))
                    .with_items(start),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
