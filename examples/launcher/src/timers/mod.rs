//! Timers and a stopwatch, kept in `timers.json` so they go on counting
//! while the launcher is closed.
//!
//! Every moment is wall-clock Unix milliseconds: a timer that ended while the
//! launcher was not running is found ended at the next start and alerts
//! then. When a timer ends a small alert opens in the corner of the screen,
//! without taking the focus, and stays until it is dismissed, restarted or
//! extended.

mod alert;
pub mod duration;
mod page;
mod stopwatch;

use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use chrono::{DateTime, Local, TimeZone as _};
use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use serde::{Deserialize, Serialize};

pub use page::{running_timers_page, start_timer_page};
pub use stopwatch::{Stopwatch, stopwatch_page};

use crate::{
    extensions::CommandId,
    model::{Action, Effect, Item, PushHandler},
    sources::system::command_item,
};

/// How often running timers are looked at, and pages showing them redrawn.
const TICK: Duration = Duration::from_secs(1);
/// The most recently used lengths remembered.
const RECENT_LIMIT: usize = 5;

pub fn now() -> i64 {
    Local::now().timestamp_millis()
}

fn local_time(at: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(at).single()
}

/// When something happens, briefly: `15:42` today, `Tomorrow 09:00` later.
pub fn format_time(at: i64, now: i64) -> String {
    let (Some(at), Some(now)) = (local_time(at), local_time(now)) else {
        return String::new();
    };
    match at.date_naive() == now.date_naive() {
        true => at.format("%H:%M").to_string(),
        false => crate::reminders::format_due(at, now),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase", tag = "state")]
pub enum TimerState {
    /// Counting down; ended once `ends_at` has passed.
    Running { ends_at: i64 },
    /// Stopped with `remaining` milliseconds to go.
    Paused { remaining: i64 },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Timer {
    pub id: String,
    /// What the user called it; empty for an unnamed timer.
    #[serde(default)]
    pub name: String,
    /// The length it was started with, in milliseconds, which Restart counts
    /// down again.
    pub length: i64,
    #[serde(flatten)]
    pub state: TimerState,
    /// Whether its alert has been shown since it ended.
    #[serde(default)]
    pub alerted: bool,
}

impl Timer {
    pub fn new(id: String, name: String, length: i64, now: i64) -> Self {
        Self {
            id,
            name,
            length,
            state: TimerState::Running {
                ends_at: now + length,
            },
            alerted: false,
        }
    }

    /// The name shown: the user's, or the length's, `25-minute timer`.
    pub fn title(&self) -> String {
        match self.name.trim().is_empty() {
            true => duration::default_name((self.length / 1000).max(1) as u64),
            false => self.name.trim().to_owned(),
        }
    }

    /// Milliseconds left, never below zero.
    pub fn remaining(&self, now: i64) -> i64 {
        match self.state {
            TimerState::Running { ends_at } => (ends_at - now).max(0),
            TimerState::Paused { remaining } => remaining.max(0),
        }
    }

    /// Whole seconds left as a countdown shows them, rounded up so it reads
    /// `0:00` only once the timer has ended.
    pub fn remaining_seconds(&self, now: i64) -> u64 {
        (self.remaining(now) as u64).div_ceil(1000)
    }

    pub fn is_paused(&self) -> bool {
        matches!(self.state, TimerState::Paused { .. })
    }

    pub fn is_ended(&self, now: i64) -> bool {
        matches!(self.state, TimerState::Running { ends_at } if ends_at <= now)
    }

    /// When it ends or ended, while it runs.
    pub fn ends_at(&self) -> Option<i64> {
        match self.state {
            TimerState::Running { ends_at } => Some(ends_at),
            TimerState::Paused { .. } => None,
        }
    }

    pub fn pause(&mut self, now: i64) {
        if let TimerState::Running { ends_at } = self.state
            && ends_at > now
        {
            self.state = TimerState::Paused {
                remaining: ends_at - now,
            };
        }
    }

    pub fn resume(&mut self, now: i64) {
        if let TimerState::Paused { remaining } = self.state {
            self.state = TimerState::Running {
                ends_at: now + remaining,
            };
        }
    }

    /// Counts its whole length down again from now.
    pub fn restart(&mut self, now: i64) {
        self.state = TimerState::Running {
            ends_at: now + self.length,
        };
        self.alerted = false;
    }

    /// Adds `extra` milliseconds; an ended timer runs again for that long.
    pub fn extend(&mut self, extra: i64, now: i64) {
        self.state = match self.state {
            TimerState::Running { ends_at } if ends_at <= now => {
                self.alerted = false;
                TimerState::Running {
                    ends_at: now + extra,
                }
            }
            TimerState::Running { ends_at } => TimerState::Running {
                ends_at: ends_at + extra,
            },
            TimerState::Paused { remaining } => TimerState::Paused {
                remaining: remaining + extra,
            },
        };
    }
}

/// What `timers.json` holds.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct Saved {
    #[serde(default)]
    timers: Vec<Timer>,
    /// Lengths in seconds, most recently started first.
    #[serde(default)]
    recent: Vec<u64>,
    #[serde(default)]
    stopwatch: Stopwatch,
}

fn path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("timers.json"))
}

fn load(file: Option<&std::path::Path>) -> Saved {
    file.and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(saved: &Saved, file: Option<&std::path::Path>) -> Result<()> {
    let path = file.ok_or_else(|| anyhow::anyhow!("no data directory"))?;
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_string_pretty(saved)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn new_id() -> String {
    let mut bytes = [0u8; 6];
    if getrandom::getrandom(&mut bytes).is_err() {
        return now().to_string();
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub struct Timers {
    saved: Saved,
    /// Where they are saved: `timers.json`, or a test's own file.
    file: Option<PathBuf>,
    /// The Running Timers subtitle last shown in the root search.
    subtitle: Option<String>,
    is_ticking: bool,
    _tick: Option<Task<()>>,
}

struct GlobalTimers(Entity<Timers>);

impl Global for GlobalTimers {}

/// The timers, read from disk on first use, which also starts watching the
/// running ones.
pub fn store(cx: &mut App) -> Entity<Timers> {
    if let Some(store) = cx.try_global::<GlobalTimers>() {
        return store.0.clone();
    }
    let file = path();
    let store = cx.new(|_| Timers {
        saved: load(file.as_deref()),
        file,
        subtitle: None,
        is_ticking: false,
        _tick: None,
    });
    cx.set_global(GlobalTimers(store.clone()));
    store.update(cx, |timers, cx| timers.check(cx));
    store
}

/// Looks for timers that ended while the launcher was closed, and watches
/// the running ones, as the launcher starts.
pub fn start(cx: &mut App) {
    store(cx);
}

/// Runs `update` on the timers store.
pub fn update(cx: &mut App, update: impl FnOnce(&mut Timers, &mut Context<Timers>)) {
    store(cx).update(cx, update);
}

impl Timers {
    pub fn timers(&self) -> &[Timer] {
        &self.saved.timers
    }

    /// Lengths in seconds, most recently started first.
    pub fn recent(&self) -> &[u64] {
        &self.saved.recent
    }

    pub fn stopwatch(&self) -> &Stopwatch {
        &self.saved.stopwatch
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = save(&self.saved, self.file.as_deref()) {
            tracing::warn!("cannot save timers: {error:#}");
        }
        cx.notify();
        self.check(cx);
    }

    /// Starts a timer of `seconds` called `name`, and remembers the length.
    pub fn start_timer(&mut self, seconds: u64, name: String, cx: &mut Context<Self>) -> Timer {
        let timer = Timer::new(new_id(), name, seconds as i64 * 1000, now());
        self.saved.timers.push(timer.clone());
        self.saved.recent.retain(|&known| known != seconds);
        self.saved.recent.insert(0, seconds);
        self.saved.recent.truncate(RECENT_LIMIT);
        self.changed(cx);
        timer
    }

    /// Changes the timer with `id`, then saves.
    pub fn change(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut Timer, i64),
        cx: &mut Context<Self>,
    ) {
        if let Some(timer) = self.saved.timers.iter_mut().find(|timer| timer.id == id) {
            change(timer, now());
            self.changed(cx);
        }
    }

    /// Stops and forgets the timer with `id`.
    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.saved.timers.retain(|timer| timer.id != id);
        self.changed(cx);
    }

    /// Changes the stopwatch, then saves.
    pub fn change_stopwatch(
        &mut self,
        change: impl FnOnce(&mut Stopwatch, i64),
        cx: &mut Context<Self>,
    ) {
        change(&mut self.saved.stopwatch, now());
        self.changed(cx);
    }

    /// Timers that have ended, soonest ended first.
    pub fn ended(&self, now: i64) -> Vec<Timer> {
        let mut ended: Vec<Timer> = self
            .saved
            .timers
            .iter()
            .filter(|timer| timer.is_ended(now))
            .cloned()
            .collect();
        ended.sort_by_key(|timer| timer.ends_at());
        ended
    }

    /// Alerts for timers that have ended, updates the root search subtitle,
    /// and keeps ticking while any timer counts down.
    fn check(&mut self, cx: &mut Context<Self>) {
        let now = now();
        let subtitle = subtitle(&self.saved.timers, now);
        if subtitle != self.subtitle {
            self.subtitle = subtitle.clone();
            cx.defer(move |cx| set_subtitle(subtitle, cx));
        }
        let is_counting =
            self.saved.timers.iter().any(
                |timer| matches!(timer.state, TimerState::Running { ends_at } if ends_at > now),
            );
        if is_counting && !self.is_ticking {
            self.is_ticking = true;
            self._tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    let is_ticking = this
                        .update(cx, |timers, cx| {
                            timers.check(cx);
                            timers.is_ticking
                        })
                        .unwrap_or(false);
                    if !is_ticking {
                        break;
                    }
                }
            }));
        } else if !is_counting {
            // The loop ends at its next tick.
            self.is_ticking = false;
        }

        let fresh = self
            .saved
            .timers
            .iter()
            .any(|timer| timer.is_ended(now) && !timer.alerted);
        if !fresh {
            return;
        }
        for timer in &mut self.saved.timers {
            if timer.is_ended(now) {
                timer.alerted = true;
            }
        }
        if let Err(error) = save(&self.saved, self.file.as_deref()) {
            tracing::warn!("cannot save timers: {error:#}");
        }
        cx.notify();
        let ended = self.ended(now);
        cx.defer(move |cx| alert::show(ended, cx));
    }
}

/// The Running Timers subtitle: the soonest to end, `Tea · 12:03 left`.
fn subtitle(timers: &[Timer], now: i64) -> Option<String> {
    let running = timers
        .iter()
        .filter(|timer| !timer.is_ended(now) && !timer.is_paused())
        .min_by_key(|timer| timer.remaining(now));
    if let Some(timer) = running {
        return Some(format!(
            "{} · {} left",
            timer.title(),
            duration::format_clock(timer.remaining_seconds(now))
        ));
    }
    if let Some(timer) = timers.iter().find(|timer| timer.is_ended(now)) {
        return Some(format!("{} · Done", timer.title()));
    }
    timers
        .iter()
        .find(|timer| timer.is_paused())
        .map(|timer| format!("{} · Paused", timer.title()))
}

fn set_subtitle(subtitle: Option<String>, cx: &mut App) {
    let Some((_, host)) = crate::shell::launcher::host_and_catalog(cx) else {
        return;
    };
    host.set_command_subtitle(
        CommandId::new("system", "running-timers"),
        subtitle.map(SharedString::from),
        cx,
    );
}

/// The root search commands this module offers on this platform.
pub fn commands() -> Vec<Item> {
    vec![
        command_item("system/start-timer", "Start Timer", "timer")
            .with_keyword("countdown")
            .with_keyword("pomodoro")
            .with_keyword("alarm")
            .with_keyword("计时器")
            .with_keyword("倒计时")
            .with_action(Action::new(
                "Start Timer",
                Effect::Push(PushHandler::new(start_timer_page)),
            )),
        command_item("system/running-timers", "Running Timers", "hourglass")
            .with_keyword("timers")
            .with_keyword("countdown")
            .with_keyword("计时器")
            .with_action(Action::new(
                "Show Running Timers",
                Effect::Push(PushHandler::new(running_timers_page)),
            )),
        command_item("system/stopwatch", "Stopwatch", "timer-reset")
            .with_keyword("lap")
            .with_keyword("elapsed")
            .with_keyword("秒表")
            .with_action(Action::new(
                "Open Stopwatch",
                Effect::Push(PushHandler::new(stopwatch_page)),
            )),
    ]
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    const MINUTE: i64 = 60_000;

    #[test]
    fn test_timer_pauses_resumes_and_extends() {
        let start = 1_000_000;
        let mut timer = Timer::new("a".into(), "Tea".into(), 5 * MINUTE, start);
        assert_eq!(timer.remaining(start + MINUTE), 4 * MINUTE);

        timer.pause(start + MINUTE);
        assert!(timer.is_paused());
        assert_eq!(
            timer.remaining(start + 10 * MINUTE),
            4 * MINUTE,
            "a paused timer keeps its time"
        );
        assert!(!timer.is_ended(start + 10 * MINUTE));

        timer.resume(start + 10 * MINUTE);
        assert_eq!(timer.ends_at(), Some(start + 14 * MINUTE));

        timer.extend(MINUTE, start + 10 * MINUTE);
        assert_eq!(timer.ends_at(), Some(start + 15 * MINUTE));
        timer.pause(start + 11 * MINUTE);
        timer.extend(5 * MINUTE, start + 11 * MINUTE);
        assert_eq!(timer.remaining(start + 11 * MINUTE), 9 * MINUTE);
    }

    #[test]
    fn test_timer_ends_and_restarts() {
        let start = 1_000_000;
        let mut timer = Timer::new("a".into(), String::new(), 25 * MINUTE, start);
        assert_eq!(timer.title(), "25-minute timer");
        assert!(!timer.is_ended(start + 25 * MINUTE - 1));
        assert_eq!(timer.remaining_seconds(start + 25 * MINUTE - 1), 1);
        let end = start + 25 * MINUTE;
        assert!(timer.is_ended(end));
        assert_eq!(timer.remaining(end + MINUTE), 0);

        timer.pause(end + MINUTE);
        assert!(!timer.is_paused(), "an ended timer cannot be paused");

        timer.alerted = true;
        timer.extend(5 * MINUTE, end + MINUTE);
        assert_eq!(timer.ends_at(), Some(end + 6 * MINUTE), "extended from now");
        assert!(!timer.alerted, "it alerts again");

        timer.alerted = true;
        timer.restart(end + 2 * MINUTE);
        assert_eq!(timer.ends_at(), Some(end + 27 * MINUTE));
        assert!(!timer.alerted);
    }

    #[test]
    fn test_subtitle_names_the_soonest() {
        let now = 1_000_000;
        let mut paused = Timer::new("p".into(), "Laundry".into(), MINUTE, now);
        paused.pause(now);
        let timers = vec![
            paused,
            Timer::new("a".into(), "Pasta".into(), 20 * MINUTE, now),
            Timer::new("b".into(), "Tea".into(), 12 * MINUTE + 3_000, now),
        ];
        assert_eq!(subtitle(&timers, now).as_deref(), Some("Tea · 12:03 left"));
        assert_eq!(
            subtitle(&timers[..1], now).as_deref(),
            Some("Laundry · Paused")
        );
        assert_eq!(subtitle(&[], now), None);
    }

    #[test]
    fn test_saves_and_loads() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("timers.json");
        let mut paused = Timer::new("p".into(), "Laundry".into(), MINUTE, 5);
        paused.pause(10);
        let mut stopwatch = Stopwatch::default();
        stopwatch.start(100);
        stopwatch.lap(2_100);
        let saved = Saved {
            timers: vec![Timer::new("a".into(), "Tea".into(), 3 * MINUTE, 5), paused],
            recent: vec![180, 1500],
            stopwatch,
        };
        save(&saved, Some(&file)).unwrap();
        assert_eq!(load(Some(&file)), saved);
        assert_eq!(
            load(Some(&folder.path().join("missing.json"))),
            Saved::default()
        );
    }

    fn store_with(timers: Vec<Timer>, file: PathBuf, cx: &mut App) -> Entity<Timers> {
        let store = cx.new(|_| Timers {
            saved: Saved {
                timers,
                ..Saved::default()
            },
            file: Some(file),
            subtitle: None,
            is_ticking: false,
            _tick: None,
        });
        cx.set_global(GlobalTimers(store.clone()));
        store
    }

    /// The alert's buttons, as a click runs them, on a store saved to a
    /// temporary file.
    #[gpui::test]
    fn test_alert_buttons_dismiss_restart_and_extend(cx: &mut TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("timers.json");
        let past = now() - MINUTE;
        cx.update(|cx| {
            gpui_kit::init(cx);
            let store = store_with(
                vec![
                    Timer::new("a".into(), "Tea".into(), MINUTE, past - 2 * MINUTE),
                    Timer::new("b".into(), "Pasta".into(), MINUTE, past - MINUTE),
                    Timer::new("c".into(), "Bread".into(), MINUTE, past - MINUTE / 2),
                ],
                file.clone(),
                cx,
            );
            // The launcher starting with ended timers: they alert.
            store.update(cx, |timers, cx| timers.check(cx));
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| alert::shown(cx)), Some("a".to_owned()));
        let saved = load(Some(&file));
        assert!(
            saved.timers.iter().all(|timer| timer.alerted),
            "alerting is saved"
        );

        cx.update(|cx| alert::dismiss("a".into(), cx));
        cx.run_until_parked();
        let saved = load(Some(&file));
        assert_eq!(saved.timers.len(), 2, "dismissing removes the timer");
        assert_eq!(cx.update(|cx| alert::shown(cx)), Some("b".to_owned()));

        cx.update(|cx| alert::restart("b".into(), cx));
        cx.run_until_parked();
        let saved = load(Some(&file));
        let left = saved.timers[0].ends_at().unwrap() - now();
        assert!((MINUTE - 2_000..=MINUTE).contains(&left), "a minute again");
        assert_eq!(cx.update(|cx| alert::shown(cx)), Some("c".to_owned()));

        cx.update(|cx| alert::extend("c".into(), cx));
        cx.run_until_parked();
        let saved = load(Some(&file));
        let left = saved.timers[1].ends_at().unwrap() - now();
        assert!(
            (5 * MINUTE - 2_000..=5 * MINUTE).contains(&left),
            "five more minutes"
        );
        assert!(!saved.timers[1].alerted, "it alerts again then");
        assert_eq!(cx.update(|cx| alert::shown(cx)), None, "nothing has ended");
    }
}
