//! My Schedule: the events of the calendars the user subscribed to, day by
//! day, with a button to join the meeting a link in the event points to.
//!
//! Calendars are iCalendar feeds: the secret address Google Calendar and
//! Outlook give each calendar, or a `.ics` file. They are fetched in the
//! background and cached, so the schedule opens at once and works offline
//! with what was fetched last.

mod ics;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Local, NaiveDate};
use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task, Window};

pub use ics::Occurrence;

use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, DetailModel, Effect, Image,
        Item, ItemId, ListModel, Metadata, MetadataValue, PageModel, PushHandler, RunHandler,
        Section, Tone,
    },
    pages::{self, Page, PageHandle},
};

/// How often feeds are fetched again while the launcher is used.
const REFRESH: Duration = Duration::from_secs(15 * 60);
/// How many days ahead the schedule lists.
const DAYS_AHEAD: i64 = 14;
/// A meeting this close is offered in the root search.
const SOON: chrono::Duration = chrono::Duration::minutes(15);

/// The events of every feed, as last fetched.
pub struct Schedule {
    occurrences: Vec<Occurrence>,
    errors: Vec<String>,
    fetched_at: Option<Instant>,
    task: Option<Task<()>>,
}

struct GlobalSchedule(Entity<Schedule>);

impl Global for GlobalSchedule {}

fn cache_directory() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("calendars"))
}

/// The cache file of the feed at `source`.
fn cache_file(source: &str) -> Option<PathBuf> {
    use std::hash::{DefaultHasher, Hash as _, Hasher as _};
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    cache_directory().map(|directory| directory.join(format!("{:016x}.ics", hasher.finish())))
}

/// Reads a feed: a web address is fetched (and cached), a path read.
fn fetch(source: &str) -> Result<String> {
    let source = source.trim();
    let url = source
        .strip_prefix("webcal://")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or_else(|| source.to_owned());
    if url.starts_with("http://") || url.starts_with("https://") {
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?
            .get(&url)
            .send()
            .with_context(|| format!("cannot reach {}", host(&url)))?
            .error_for_status()
            .with_context(|| format!("{} refused the calendar", host(&url)))?;
        let text = response.text()?;
        if !text.contains("BEGIN:VCALENDAR") {
            anyhow::bail!("{} did not send a calendar", host(&url));
        }
        if let Some(cache) = cache_file(source) {
            if let Some(directory) = cache.parent() {
                std::fs::create_dir_all(directory).ok();
            }
            std::fs::write(cache, &text).ok();
        }
        Ok(text)
    } else {
        std::fs::read_to_string(crate::shell::settings::expand_home(source))
            .with_context(|| format!("cannot read {source}"))
    }
}

fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "the server".to_owned())
}

/// Every feed's occurrences in the coming days; a feed that cannot be
/// fetched falls back to its cached copy and is reported.
fn load(sources: &[String]) -> (Vec<Occurrence>, Vec<String>) {
    let today = Local::now().date_naive();
    let from = local_day(today);
    let to = local_day(today + chrono::Duration::days(DAYS_AHEAD));
    let mut occurrences = Vec::new();
    let mut errors = Vec::new();
    for source in sources {
        let text = match fetch(source) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{error:#}"));
                match cache_file(source).and_then(|cache| std::fs::read_to_string(cache).ok()) {
                    Some(text) => text,
                    None => continue,
                }
            }
        };
        occurrences.extend(ics::occurrences(&text, from, to));
    }
    occurrences.sort_by(|a, b| a.start.cmp(&b.start).then(a.title.cmp(&b.title)));
    occurrences.dedup_by(|a, b| a.uid == b.uid && a.start == b.start);
    (occurrences, errors)
}

fn local_day(date: NaiveDate) -> DateTime<Local> {
    use chrono::TimeZone as _;
    Local
        .from_local_datetime(&date.and_time(chrono::NaiveTime::MIN))
        .earliest()
        .unwrap_or_else(Local::now)
}

/// The schedule, created on first use.
pub fn schedule(cx: &mut App) -> Entity<Schedule> {
    if let Some(schedule) = cx.try_global::<GlobalSchedule>() {
        return schedule.0.clone();
    }
    let schedule = cx.new(|_| Schedule {
        occurrences: Vec::new(),
        errors: Vec::new(),
        fetched_at: None,
        task: None,
    });
    cx.set_global(GlobalSchedule(schedule.clone()));
    schedule
}

/// Fetches the feeds when the launcher starts, if any are set.
pub fn start(cx: &mut App) {
    if !crate::shell::launcher::settings(cx)
        .calendar_feeds()
        .is_empty()
    {
        let schedule = schedule(cx);
        schedule.update(cx, |schedule, cx| schedule.refresh(false, cx));
    }
}

impl Schedule {
    /// Fetches the feeds again, unless they were fetched recently.
    pub fn refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        let recent = self
            .fetched_at
            .is_some_and(|fetched_at| fetched_at.elapsed() < REFRESH);
        // A forced refresh replaces a fetch under way, which may be reading
        // feeds that were since removed; dropping its task cancels it.
        if !force && (recent || self.task.is_some()) {
            return;
        }
        let sources: Vec<String> = crate::shell::launcher::settings(cx)
            .calendar_feeds()
            .to_vec();
        self.task = Some(cx.spawn(async move |this, cx| {
            let (occurrences, errors) = cx.background_spawn(async move { load(&sources) }).await;
            this.update(cx, |schedule, cx| {
                schedule.occurrences = occurrences;
                schedule.errors = errors;
                schedule.fetched_at = Some(Instant::now());
                schedule.task = None;
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn is_loading(&self) -> bool {
        self.task.is_some()
    }

    /// The meeting starting soon or under way that has a link to join, for
    /// the root search.
    pub fn next_meeting(&self) -> Option<&Occurrence> {
        let now = Local::now();
        self.occurrences.iter().find(|occurrence| {
            !occurrence.all_day
                && occurrence.end > now
                && occurrence.start - SOON <= now
                && meeting_link(occurrence).is_some()
        })
    }
}

/// The video call an event links to: Zoom, Meet, Teams, Webex and the like.
pub fn meeting_link(occurrence: &Occurrence) -> Option<String> {
    const SERVICES: [&str; 7] = [
        "zoom.us/",
        "meet.google.com/",
        "teams.microsoft.com/",
        "teams.live.com/",
        "webex.com/",
        "meeting.tencent.com/",
        "feishu.cn/",
    ];
    [
        &occurrence.url,
        &occurrence.location,
        &occurrence.description,
    ]
    .into_iter()
    .flatten()
    .flat_map(|text| text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"')))
    .map(|word| word.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | '.')))
    .find(|word| {
        word.starts_with("https://") && SERVICES.iter().any(|service| word.contains(service))
    })
    .map(str::to_owned)
}

fn time_range(occurrence: &Occurrence) -> String {
    match occurrence.all_day {
        true => "All day".to_owned(),
        false => format!(
            "{} – {}",
            occurrence.start.format("%H:%M"),
            occurrence.end.format("%H:%M")
        ),
    }
}

/// “in 10 min”, “now”, or nothing for events further away.
fn countdown(occurrence: &Occurrence, now: DateTime<Local>) -> Option<(String, Tone)> {
    if occurrence.all_day {
        return None;
    }
    if occurrence.start <= now && now < occurrence.end {
        return Some(("Now".to_owned(), Tone::Success));
    }
    let minutes = (occurrence.start - now).num_minutes();
    match minutes {
        0..=59 => Some((format!("in {} min", minutes.max(1)), Tone::Warning)),
        60..=479 => Some((format!("in {} h", minutes / 60), Tone::Neutral)),
        _ => None,
    }
}

/// An event as a row: its time, where it is, and the meeting to join.
pub fn occurrence_item(occurrence: &Occurrence, now: DateTime<Local>) -> Item {
    let link = meeting_link(occurrence);
    let mut actions = ActionPanel::new();
    if let Some(link) = &link {
        actions = actions.with_action(
            Action::new("Join Meeting", Effect::OpenUrl(link.clone().into()))
                .with_image(Image::Icon("video".into())),
        );
    }
    actions = actions.with_action(
        Action::new(
            "Copy Event",
            Effect::Copy(
                format!(
                    "{} · {} {}",
                    occurrence.title,
                    occurrence.start.format("%Y-%m-%d"),
                    time_range(occurrence)
                )
                .into(),
            ),
        )
        .with_image(Image::Icon("copy".into()))
        .with_shortcut("secondary-shift-c"),
    );
    if let Some(link) = link {
        actions = actions.with_action(
            Action::new("Copy Meeting Link", Effect::Copy(link.into()))
                .with_image(Image::Icon("link".into())),
        );
    }
    if let Some(url) = &occurrence.url
        && url.starts_with("http")
    {
        actions = actions.with_section(
            ActionSection::new().with_entry(ActionEntry::Action(
                Action::new("Open Event", Effect::OpenUrl(url.clone().into()))
                    .with_image(Image::Icon("external-link".into())),
            )),
        );
    }
    let past = occurrence.end <= now;
    let mut detail = DetailModel::new(
        occurrence
            .description
            .clone()
            .unwrap_or_default()
            .chars()
            .take(3000)
            .collect::<String>(),
    )
    .with_metadata(Metadata::new(
        "When",
        MetadataValue::Text(
            format!(
                "{} {}",
                occurrence.start.format("%a %b %-d"),
                time_range(occurrence)
            )
            .into(),
        ),
    ));
    if let Some(location) = &occurrence.location {
        detail = detail.with_metadata(Metadata::new(
            "Where",
            MetadataValue::Text(location.clone().into()),
        ));
    }
    if let Some(calendar) = &occurrence.calendar {
        detail = detail.with_metadata(Metadata::new(
            "Calendar",
            MetadataValue::Text(calendar.clone().into()),
        ));
    }
    let item = Item::new(
        ItemId::new(format!(
            "event:{}:{}",
            occurrence.uid,
            occurrence.start.timestamp()
        )),
        occurrence.title.clone(),
    )
    .with_subtitle(time_range(occurrence))
    .with_image(Image::Icon(
        match (meeting_link(occurrence).is_some(), past) {
            (_, true) => "calendar-check",
            (true, false) => "video",
            (false, false) => "calendar",
        }
        .into(),
    ))
    .with_keyword(occurrence.location.clone().unwrap_or_default())
    .with_keyword(occurrence.calendar.clone().unwrap_or_default())
    .with_detail(detail)
    .with_actions(actions);
    match countdown(occurrence, now) {
        Some((text, tone)) => item.with_accessory(Accessory::tag(text, tone)),
        None => item,
    }
}

fn day_title(date: NaiveDate, today: NaiveDate) -> String {
    match (date - today).num_days() {
        0 => format!("Today · {}", date.format("%a %b %-d")),
        1 => format!("Tomorrow · {}", date.format("%a %b %-d")),
        _ => date.format("%A, %b %-d").to_string(),
    }
}

pub fn my_schedule_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let schedule = schedule(cx);
    schedule.update(cx, |schedule, cx| schedule.refresh(false, cx));
    Ok(pages::handle(cx.new(|cx| {
        let subscription = cx.observe(&schedule, |_, _, cx| cx.notify());
        SchedulePage {
            schedule,
            _subscription: subscription,
        }
    })))
}

struct SchedulePage {
    schedule: Entity<Schedule>,
    _subscription: gpui_kit::Subscription,
}

impl Page for SchedulePage {
    fn title(&self) -> SharedString {
        "My Schedule".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let feeds = crate::shell::launcher::settings(cx).calendar_feeds().len();
        let schedule = self.schedule.read(cx);
        let now = Local::now();
        let today = now.date_naive();
        let refresh = {
            let schedule = self.schedule.clone();
            Action::new(
                "Refresh Calendars",
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    schedule.update(cx, |schedule, cx| schedule.refresh(true, cx));
                })),
            )
            .with_image(Image::Icon("refresh-cw".into()))
            .with_shortcut("secondary-r")
        };
        let settings = Action::new(
            "Add Calendars in Settings",
            Effect::Push(PushHandler::new(crate::shell::settings::settings_page)),
        )
        .with_image(Image::Icon("settings".into()))
        .with_shortcut("secondary-comma");

        let mut sections: Vec<(NaiveDate, Vec<Item>)> = Vec::new();
        for occurrence in &schedule.occurrences {
            // Today's finished events stay, dimmed by their icon; earlier
            // days are gone.
            let date = occurrence.start.date_naive().max(today);
            if occurrence.end < now && date != today {
                continue;
            }
            let item = occurrence_item(occurrence, now);
            let item = item.clone().with_actions(
                item.actions().clone().with_section(
                    ActionSection::new()
                        .with_entry(ActionEntry::Action(refresh.clone()))
                        .with_entry(ActionEntry::Action(settings.clone())),
                ),
            );
            match sections.last_mut() {
                Some((day, items)) if *day == date => items.push(item),
                _ => sections.push((date, vec![item])),
            }
        }
        let (empty_title, empty_description) = match (feeds, schedule.errors.first()) {
            (0, _) => (
                "No calendars yet".to_owned(),
                "Add your calendar’s iCal address (Google Calendar: Settings → Integrate \
                 calendar → Secret address) in Launcher Settings."
                    .to_owned(),
            ),
            (_, Some(error)) => ("Couldn’t load your calendars".to_owned(), error.clone()),
            (_, None) => (
                "Nothing scheduled".to_owned(),
                format!("No events in the next {DAYS_AHEAD} days."),
            ),
        };
        let add = (feeds == 0).then(|| {
            Item::new(ItemId::new("add-calendars"), "Add Calendars")
                .with_icon("calendar-plus")
                .with_subtitle("Subscribe to Google Calendar or Outlook in Settings")
                .with_action(settings.clone())
        });
        sections
            .into_iter()
            .fold(
                ListModel::new()
                    .with_section(Section::new().with_items(add))
                    .with_placeholder("Search events…")
                    .with_loading(schedule.is_loading())
                    .with_showing_detail(!schedule.occurrences.is_empty())
                    .with_empty_title(empty_title)
                    .with_empty_description(empty_description),
                |list, (date, items)| {
                    list.with_section(
                        Section::new()
                            .with_title(day_title(date, today))
                            .with_items(items),
                    )
                },
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

/// The root search command.
pub fn commands() -> Vec<Item> {
    vec![
        Item::new(ItemId::new("system/my-schedule"), "My Schedule")
            .with_icon("calendar")
            .with_accessory(Accessory::text("Command"))
            .with_keyword("calendar")
            .with_keyword("events")
            .with_keyword("meeting")
            .with_action(Action::new(
                "My Schedule",
                Effect::Push(PushHandler::new(my_schedule_page)),
            )),
    ]
}

#[cfg(test)]
mod tests {
    /// Fetches `ICS_FEED`, when set, to try a real calendar by hand.
    #[test]
    #[ignore = "needs the network"]
    fn test_load_feed() {
        let feed = std::env::var("ICS_FEED").unwrap();
        let (occurrences, errors) = super::load(&[feed]);
        println!("errors: {errors:?}");
        for occurrence in occurrences.iter().take(5) {
            println!("{} {}", occurrence.start, occurrence.title);
        }
    }
}
