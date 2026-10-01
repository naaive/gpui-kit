//! The stopwatch: started, paused, lapped and reset from its page, and kept
//! running while the page and the launcher are closed.

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Subscription, Task, Window};
use serde::{Deserialize, Serialize};

use super::{TICK, Timers, duration::format_clock, now, store, update};
use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Effect, Image,
        Item, ItemId, ListModel, PageModel, RunHandler, Section,
    },
    pages::{self, Page, PageHandle},
};

/// Elapsed time is wall-clock: a stopwatch started before a restart of the
/// launcher or the computer has kept counting.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Stopwatch {
    /// When the current run started, Unix milliseconds; `None` while paused.
    #[serde(default)]
    started_at: Option<i64>,
    /// Milliseconds counted by earlier runs.
    #[serde(default)]
    counted: i64,
    /// The elapsed time at each lap, in milliseconds, in the order taken.
    #[serde(default)]
    laps: Vec<i64>,
}

impl Stopwatch {
    pub fn is_running(&self) -> bool {
        self.started_at.is_some()
    }

    /// Whether it shows nothing: never started, or reset.
    pub fn is_reset(&self) -> bool {
        !self.is_running() && self.counted == 0 && self.laps.is_empty()
    }

    pub fn elapsed(&self, now: i64) -> i64 {
        self.counted + self.started_at.map_or(0, |started| (now - started).max(0))
    }

    pub fn start(&mut self, now: i64) {
        if self.started_at.is_none() {
            self.started_at = Some(now);
        }
    }

    pub fn pause(&mut self, now: i64) {
        self.counted = self.elapsed(now);
        self.started_at = None;
    }

    pub fn lap(&mut self, now: i64) {
        if self.is_running() {
            self.laps.push(self.elapsed(now));
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Each lap's own length and the elapsed time at its end, newest first,
    /// numbered from 1.
    pub fn laps(&self) -> Vec<(usize, i64, i64)> {
        let mut previous = 0;
        let mut laps: Vec<(usize, i64, i64)> = self
            .laps
            .iter()
            .enumerate()
            .map(|(ix, &at)| {
                let lap = (ix + 1, at - previous, at);
                previous = at;
                lap
            })
            .collect();
        laps.reverse();
        laps
    }
}

fn clock(milliseconds: i64) -> String {
    format_clock((milliseconds.max(0) / 1000) as u64)
}

pub fn stopwatch_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let store = store(cx);
    Ok(pages::handle(cx.new(|cx| StopwatchPage {
        _subscription: cx.observe(&store, |_, _, cx| cx.notify()),
        _tick: cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this
                    .update(cx, |page: &mut StopwatchPage, cx| {
                        if page.store.read(cx).stopwatch().is_running() {
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }),
        store,
    })))
}

struct StopwatchPage {
    store: Entity<Timers>,
    _subscription: Subscription,
    _tick: Task<()>,
}

fn run(change: fn(&mut Stopwatch, i64)) -> Effect {
    Effect::Run(RunHandler::new(move |(), _, cx| {
        update(cx, |timers, cx| timers.change_stopwatch(change, cx))
    }))
}

fn actions(stopwatch: &Stopwatch, copy: Option<String>) -> ActionPanel {
    let toggle = match stopwatch.is_running() {
        true => Action::new("Pause", run(Stopwatch::pause)).with_image(Image::Icon("pause".into())),
        false => Action::new(
            match stopwatch.is_reset() {
                true => "Start",
                false => "Resume",
            },
            run(Stopwatch::start),
        )
        .with_image(Image::Icon("play".into())),
    };
    let mut actions = ActionPanel::new().with_action(toggle);
    if stopwatch.is_running() {
        actions = actions.with_action(
            Action::new("Lap", run(Stopwatch::lap))
                .with_image(Image::Icon("flag".into()))
                .with_shortcut("secondary-l"),
        );
    }
    if let Some(copy) = copy {
        actions = actions.with_action(
            Action::new("Copy Time", Effect::Copy(copy.into()))
                .with_image(Image::Icon("copy".into()))
                .with_shortcut("secondary-shift-c"),
        );
    }
    if !stopwatch.is_reset() {
        actions = actions.with_section(
            ActionSection::new().with_entry(ActionEntry::Action(
                Action::new("Reset", run(|stopwatch, _| stopwatch.reset()))
                    .with_image(Image::Icon("rotate-ccw".into()))
                    .with_style(ActionStyle::Destructive)
                    .with_shortcut("ctrl-x"),
            )),
        );
    }
    actions
}

impl Page for StopwatchPage {
    fn title(&self) -> SharedString {
        "Stopwatch".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let now = now();
        let stopwatch = self.store.read(cx).stopwatch().clone();
        let elapsed = clock(stopwatch.elapsed(now));
        let state = match (stopwatch.is_running(), stopwatch.is_reset()) {
            (true, _) => Accessory::tag("Running", crate::model::Tone::Success),
            (false, true) => Accessory::text("Not started"),
            (false, false) => Accessory::tag("Paused", crate::model::Tone::Neutral),
        };
        let main = Item::new(ItemId::new("stopwatch/elapsed"), elapsed.clone())
            .with_image(Image::Icon("timer-reset".into()))
            .with_accessory(state)
            .with_actions(actions(&stopwatch, Some(elapsed)));
        let laps = stopwatch.laps().into_iter().map(|(number, lap, at)| {
            Item::new(ItemId::new(format!("stopwatch/lap/{number}")), clock(lap))
                .with_image(Image::Icon("flag".into()))
                .with_subtitle(format!("Lap {number}"))
                .with_accessory(Accessory::text(clock(at)))
                .with_actions(actions(&stopwatch, Some(clock(lap))))
        });
        ListModel::new()
            .with_filtering(false)
            .with_placeholder("Stopwatch")
            .with_item(main)
            .with_section(Section::new().with_title("Laps").with_items(laps))
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stopwatch_counts_laps_and_resets() {
        let mut stopwatch = Stopwatch::default();
        assert!(stopwatch.is_reset());
        stopwatch.start(1_000);
        assert_eq!(stopwatch.elapsed(4_000), 3_000);
        stopwatch.lap(4_000);
        stopwatch.pause(6_000);
        assert_eq!(
            stopwatch.elapsed(60_000),
            5_000,
            "paused, it stops counting"
        );
        stopwatch.lap(60_000);
        assert_eq!(stopwatch.laps().len(), 1, "no lap while paused");

        stopwatch.start(100_000);
        stopwatch.start(200_000);
        assert_eq!(
            stopwatch.elapsed(102_000),
            7_000,
            "starting twice changes nothing"
        );
        stopwatch.lap(102_000);
        assert_eq!(stopwatch.laps(), vec![(2, 4_000, 7_000), (1, 3_000, 3_000)]);

        stopwatch.reset();
        assert!(stopwatch.is_reset());
        assert_eq!(stopwatch.elapsed(200_000), 0);
    }
}
