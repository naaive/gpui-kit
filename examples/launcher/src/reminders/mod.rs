//! Reminders: things to do, each optionally due at a time, kept in
//! `reminders.json`. When one comes due a small alert opens in the corner of
//! the screen, without taking the focus, and stays until it is completed or
//! snoozed. Due and overdue reminders also show at the top of the root
//! search.

mod alert;
pub mod due;
mod page;

use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use chrono::{DateTime, Local, TimeZone as _};
use gpui_kit::{App, AppContext as _, Context, Entity, Global, Task};
use serde::{Deserialize, Serialize};

pub use page::{create_reminder_page, search_reminders_page};

use crate::model::{
    Accessory, Action, ActionPanel, Effect, Image, Item, ItemId, PushHandler, RunHandler, Tone,
};

/// How often due reminders are looked for.
const CHECK_EVERY: Duration = Duration::from_secs(15);
/// The root search shows reminders due within this long.
const SOON: i64 = 60 * 60;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl Priority {
    pub const ALL: [Self; 4] = [Self::None, Self::Low, Self::Medium, Self::High];

    pub fn value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::None => "No Priority",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    pub fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|priority| priority.value() == value)
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Reminder {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    /// Unix seconds.
    #[serde(default)]
    pub due: Option<i64>,
    #[serde(default)]
    pub priority: Priority,
    /// Unix seconds.
    #[serde(default)]
    pub completed_at: Option<i64>,
    /// Whether its alert has been shown for the current due time.
    #[serde(default)]
    pub alerted: bool,
}

impl Reminder {
    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }

    pub fn due_time(&self) -> Option<DateTime<Local>> {
        self.due
            .and_then(|due| Local.timestamp_opt(due, 0).single())
    }

    fn is_due(&self, now: i64) -> bool {
        !self.is_completed() && self.due.is_some_and(|due| due <= now)
    }
}

fn path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("reminders.json"))
}

pub fn now() -> i64 {
    Local::now().timestamp()
}

/// When a reminder is due, briefly: `Today 14:00`, `Tomorrow 09:00`,
/// `Fri 17:00`, `Oct 3 09:00`.
pub fn format_due(due: DateTime<Local>, now: DateTime<Local>) -> String {
    let days = (due.date_naive() - now.date_naive()).num_days();
    let time = due.format("%H:%M");
    match days {
        0 => format!("Today {time}"),
        1 => format!("Tomorrow {time}"),
        -1 => format!("Yesterday {time}"),
        2..7 => format!("{} {time}", due.format("%a")),
        _ => format!("{} {time}", due.format("%b %-d")),
    }
}

pub struct Reminders {
    reminders: Vec<Reminder>,
    /// Where they are saved: `reminders.json`, or a test's own file.
    file: Option<PathBuf>,
    /// The ids `due_soon` gave at the last check, to notice it change as
    /// time passes.
    soon: Vec<String>,
    _check: Option<Task<()>>,
}

struct GlobalReminders(Entity<Reminders>);

impl Global for GlobalReminders {}

/// The reminders, read from disk on first use.
pub fn store(cx: &mut App) -> Entity<Reminders> {
    if let Some(store) = cx.try_global::<GlobalReminders>() {
        return store.0.clone();
    }
    let file = path();
    let store = cx.new(|_| Reminders {
        reminders: load(file.as_deref()),
        file,
        soon: Vec::new(),
        _check: None,
    });
    cx.set_global(GlobalReminders(store.clone()));
    store
}

fn load(file: Option<&std::path::Path>) -> Vec<Reminder> {
    file.and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Starts looking for due reminders, as the launcher starts.
pub fn start(cx: &mut App) {
    let store = store(cx);
    store.update(cx, |reminders, cx| {
        reminders._check = Some(cx.spawn(async move |this, cx| {
            loop {
                let alive = this.update(cx, |reminders, cx| reminders.check(cx)).is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(CHECK_EVERY).await;
            }
        }));
    });
}

/// Reads `reminders.json` again, after an import.
pub fn reload(cx: &mut App) {
    if let Some(store) = cx
        .try_global::<GlobalReminders>()
        .map(|store| store.0.clone())
    {
        store.update(cx, |reminders, cx| {
            reminders.reminders = load(reminders.file.as_deref());
            cx.notify();
        });
    }
}

impl Reminders {
    pub fn reminders(&self) -> &[Reminder] {
        &self.reminders
    }

    fn save(&self) -> Result<()> {
        let path = self
            .file
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no data directory"))?;
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_string_pretty(&self.reminders)?)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.save() {
            tracing::warn!("cannot save reminders: {error:#}");
        }
        cx.notify();
    }

    /// Shows the alert for reminders that have come due.
    fn check(&mut self, cx: &mut Context<Self>) {
        let soon: Vec<String> = self
            .due_soon()
            .into_iter()
            .map(|reminder| reminder.id)
            .collect();
        if soon != self.soon {
            self.soon = soon;
            cx.notify();
        }
        let now = now();
        let due: Vec<Reminder> = self
            .reminders
            .iter()
            .filter(|reminder| reminder.is_due(now) && !reminder.alerted)
            .cloned()
            .collect();
        if due.is_empty() {
            return;
        }
        for reminder in &mut self.reminders {
            if due.iter().any(|due| due.id == reminder.id) {
                reminder.alerted = true;
            }
        }
        self.changed(cx);
        let pending: Vec<Reminder> = self
            .reminders
            .iter()
            .filter(|reminder| reminder.is_due(now))
            .cloned()
            .collect();
        cx.defer(move |cx| alert::show(pending, cx));
    }

    /// Adds `reminder`, or replaces the one with its id.
    pub fn save_reminder(&mut self, mut reminder: Reminder, cx: &mut Context<Self>) {
        match self
            .reminders
            .iter_mut()
            .find(|known| known.id == reminder.id)
        {
            Some(known) => {
                // A new due time alerts again.
                reminder.alerted = known.alerted && known.due == reminder.due;
                *known = reminder;
            }
            None => self.reminders.push(reminder),
        }
        self.changed(cx);
    }

    pub fn set_completed(&mut self, id: &str, completed: bool, cx: &mut Context<Self>) {
        if let Some(reminder) = self.reminders.iter_mut().find(|known| known.id == id) {
            reminder.completed_at = completed.then(now);
            self.changed(cx);
        }
    }

    /// Makes the reminder due again `minutes` from now.
    pub fn snooze(&mut self, id: &str, minutes: i64, cx: &mut Context<Self>) {
        if let Some(reminder) = self.reminders.iter_mut().find(|known| known.id == id) {
            reminder.due = Some(now() + minutes * 60);
            reminder.alerted = false;
            self.changed(cx);
        }
    }

    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.reminders.retain(|known| known.id != id);
        self.changed(cx);
    }

    pub fn clear_completed(&mut self, cx: &mut Context<Self>) {
        self.reminders.retain(|known| !known.is_completed());
        self.changed(cx);
    }

    /// Open reminders that are overdue or due within the hour, soonest
    /// first, for the root search.
    pub fn due_soon(&self) -> Vec<Reminder> {
        let now = now();
        let mut soon: Vec<Reminder> = self
            .reminders
            .iter()
            .filter(|reminder| !reminder.is_completed())
            .filter(|reminder| reminder.due.is_some_and(|due| due <= now + SOON))
            .cloned()
            .collect();
        soon.sort_by_key(|reminder| reminder.due);
        soon
    }
}

/// Runs `update` on the reminders store.
pub fn update(cx: &mut App, update: impl FnOnce(&mut Reminders, &mut Context<Reminders>)) {
    store(cx).update(cx, update);
}

pub fn priority_accessory(priority: Priority) -> Option<Accessory> {
    match priority {
        Priority::None => None,
        Priority::Low => Some(Accessory::tag("Low", Tone::Neutral)),
        Priority::Medium => Some(Accessory::tag("Medium", Tone::Warning)),
        Priority::High => Some(Accessory::tag("High", Tone::Danger)),
    }
}

/// A due reminder for the root search: completing it is the first action.
pub fn due_item(reminder: &Reminder) -> Item {
    let now = Local::now();
    let (complete, snooze, edit) = (reminder.id.clone(), reminder.id.clone(), reminder.clone());
    let overdue = reminder.due.is_some_and(|due| due <= now.timestamp());
    let mut item = Item::new(
        ItemId::new(format!("reminder/{}", reminder.id)),
        reminder.title.clone(),
    )
    .with_image(Image::Icon("list-todo".into()))
    .with_actions(
        ActionPanel::new()
            .with_action(
                Action::new(
                    "Complete Reminder",
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        update(cx, |reminders, cx| {
                            reminders.set_completed(&complete, true, cx)
                        })
                    })),
                )
                .with_image(Image::Icon("circle-check".into())),
            )
            .with_action(
                Action::new(
                    "Snooze for 10 Minutes",
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        update(cx, |reminders, cx| reminders.snooze(&snooze, 10, cx))
                    })),
                )
                .with_image(Image::Icon("alarm-clock".into())),
            )
            .with_action(
                Action::new(
                    "Edit Reminder",
                    Effect::Push(PushHandler::new(move |window, cx| {
                        page::reminder_form(Some(edit.clone()), window, cx)
                    })),
                )
                .with_image(Image::Icon("pencil".into()))
                .with_shortcut("secondary-e"),
            ),
    );
    if let Some(due) = reminder.due_time() {
        item = item.with_accessory(match overdue {
            true => Accessory::tag(format_due(due, now), Tone::Danger),
            false => Accessory::text(format_due(due, now)),
        });
    }
    item
}

pub fn commands() -> Vec<Item> {
    vec![
        Item::new(ItemId::new("system/search-reminders"), "Search Reminders")
            .with_icon("list-todo")
            .with_accessory(Accessory::text("Command"))
            .with_keyword("todo")
            .with_keyword("tasks")
            .with_keyword("reminders")
            .with_action(Action::new(
                "Search Reminders",
                Effect::Push(PushHandler::new(search_reminders_page)),
            )),
        Item::new(ItemId::new("system/create-reminder"), "Create Reminder")
            .with_icon("alarm-clock-plus")
            .with_accessory(Accessory::text("Command"))
            .with_keyword("todo")
            .with_keyword("remind me")
            .with_keyword("task")
            .with_action(Action::new(
                "Create Reminder",
                Effect::Push(PushHandler::new(create_reminder_page)),
            )),
    ]
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    fn reminder(id: &str, due: i64) -> Reminder {
        Reminder {
            id: id.into(),
            title: format!("Reminder {id}"),
            notes: String::new(),
            due: Some(due),
            priority: Priority::None,
            completed_at: None,
            alerted: true,
        }
    }

    /// The alert's buttons, as a click runs them, on a store saved to a
    /// temporary file.
    #[gpui::test]
    fn test_alert_buttons_complete_and_snooze(cx: &mut TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("reminders.json");
        let past = now() - 60;
        cx.update(|cx| {
            gpui_kit::init(cx);
            let store = cx.new(|_| Reminders {
                reminders: vec![reminder("a", past - 60), reminder("b", past)],
                file: Some(file.clone()),
                soon: Vec::new(),
                _check: None,
            });
            cx.set_global(GlobalReminders(store.clone()));
            let due = store.read(cx).reminders().to_vec();
            alert::show(due, cx);
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| alert::shown(cx)), Some("a".to_owned()));

        cx.update(|cx| alert::complete("a".into(), cx));
        cx.run_until_parked();
        let saved: Vec<Reminder> =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(saved[0].is_completed(), "completing is saved");
        assert_eq!(
            cx.update(|cx| alert::shown(cx)),
            Some("b".to_owned()),
            "the alert moves on to what is still due"
        );

        cx.update(|cx| alert::snooze("b".into(), cx));
        cx.run_until_parked();
        let saved: Vec<Reminder> =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let snoozed = saved[1].due.unwrap() - now();
        assert!((590..=600).contains(&snoozed), "due again in ten minutes");
        assert!(!saved[1].alerted, "it alerts again then");
        assert_eq!(cx.update(|cx| alert::shown(cx)), None, "nothing is due");
    }
}
