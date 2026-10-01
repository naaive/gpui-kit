//! Reading iCalendar (`.ics`) feeds: the events, their recurrences and time
//! zones, expanded into the occurrences that fall in a range.
//!
//! What calendars publish in practice is covered: `RRULE` with `FREQ`,
//! `INTERVAL`, `COUNT`, `UNTIL`, `BYDAY` (with ordinals in monthly and
//! yearly rules), `BYMONTHDAY`, `BYMONTH`, `BYSETPOS` and `WKST`; `EXDATE`; moved or cancelled occurrences
//! (`RECURRENCE-ID`); all-day events; and `TZID`s spelled the IANA way
//! (Google) or the Windows way (Outlook).

use std::collections::HashMap;

use chrono::{
    DateTime, Datelike as _, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone as _,
    Utc, Weekday,
};

/// One occurrence of an event.
#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub uid: String,
    pub title: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
    pub location: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    /// The calendar's own name (`X-WR-CALNAME`), when it has one.
    pub calendar: Option<String>,
}

/// An event as written in the feed, before recurrences are expanded.
#[derive(Clone, Debug, Default)]
struct Event {
    uid: String,
    title: String,
    start: Option<Moment>,
    end: Option<Moment>,
    duration: Option<Duration>,
    location: Option<String>,
    description: Option<String>,
    url: Option<String>,
    rule: Option<String>,
    exceptions: Vec<Moment>,
    recurrence_id: Option<Moment>,
    cancelled: bool,
}

/// A point in time as the feed wrote it: a day, or a wall-clock time in the
/// zone it was written in, which recurrences repeat in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Moment {
    Date(NaiveDate),
    Time(NaiveDateTime, Zone),
}

/// The zone of a written time.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Zone {
    /// `…Z`: a fixed instant.
    Utc,
    Named(chrono_tz::Tz),
    /// No zone, or one not known: the computer's own.
    Floating,
}

impl Zone {
    /// The instant `time` is in this zone. A time skipped by a clock change
    /// is taken an hour later, as calendars show it.
    fn resolve(self, time: NaiveDateTime) -> DateTime<Local> {
        fn in_zone<Z: chrono::TimeZone>(zone: &Z, time: NaiveDateTime) -> Option<DateTime<Local>> {
            zone.from_local_datetime(&time)
                .earliest()
                .or_else(|| {
                    zone.from_local_datetime(&(time + Duration::hours(1)))
                        .earliest()
                })
                .map(|time| time.with_timezone(&Local))
        }
        let resolved = match self {
            Self::Utc => Some(Utc.from_utc_datetime(&time).with_timezone(&Local)),
            Self::Named(zone) => in_zone(&zone, time),
            Self::Floating => in_zone(&Local, time),
        };
        resolved.unwrap_or_else(|| Utc.from_utc_datetime(&time).with_timezone(&Local))
    }
}

impl Moment {
    fn local(self) -> DateTime<Local> {
        match self {
            Self::Date(date) => local_midnight(date),
            Self::Time(time, zone) => zone.resolve(time),
        }
    }
}

fn local_midnight(date: NaiveDate) -> DateTime<Local> {
    Zone::Floating.resolve(date.and_time(NaiveTime::MIN))
}

/// The lines of a feed, with folded continuation lines joined.
fn unfold(source: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in source.lines() {
        match (
            line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')),
            lines.last_mut(),
        ) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(line.to_owned()),
        }
    }
    lines
}

/// `DTSTART;TZID=Europe/Paris:20240101T090000` → (`DTSTART`, {TZID}, value).
fn split(line: &str) -> Option<(String, HashMap<String, String>, String)> {
    // The value starts after the first colon outside a quoted parameter.
    let mut quoted = false;
    let colon = line.char_indices().find_map(|(index, c)| match c {
        '"' => {
            quoted = !quoted;
            None
        }
        ':' if !quoted => Some(index),
        _ => None,
    })?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let parameters = parts
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.to_ascii_uppercase(), value.trim_matches('"').to_owned()))
        .collect();
    Some((name, parameters, value.to_owned()))
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Reads a date or date-time value in the zone its parameters name.
fn moment(value: &str, parameters: &HashMap<String, String>) -> Option<Moment> {
    let value = value.trim();
    if parameters.get("VALUE").is_some_and(|kind| kind == "DATE") || value.len() == 8 {
        return NaiveDate::parse_from_str(value, "%Y%m%d")
            .ok()
            .map(Moment::Date);
    }
    if let Some(utc) = value.strip_suffix('Z') {
        let time = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some(Moment::Time(time, Zone::Utc));
    }
    let time = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    let zone = match parameters.get("TZID").and_then(|zone| zone_named(zone)) {
        Some(zone) => Zone::Named(zone),
        None => Zone::Floating,
    };
    Some(Moment::Time(time, zone))
}

/// An IANA zone, or the IANA zone for a Windows zone name.
fn zone_named(name: &str) -> Option<chrono_tz::Tz> {
    let name = name.trim().trim_start_matches('/');
    if let Ok(zone) = name.parse::<chrono_tz::Tz>() {
        return Some(zone);
    }
    let iana = match name {
        "UTC" | "Coordinated Universal Time" | "GMT" => "UTC",
        "China Standard Time" => "Asia/Shanghai",
        "Taipei Standard Time" => "Asia/Taipei",
        "Tokyo Standard Time" => "Asia/Tokyo",
        "Korea Standard Time" => "Asia/Seoul",
        "Singapore Standard Time" => "Asia/Singapore",
        "India Standard Time" => "Asia/Kolkata",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Romance Standard Time" => "Europe/Paris",
        "Central Europe Standard Time" => "Europe/Budapest",
        "Central European Standard Time" => "Europe/Warsaw",
        "GMT Standard Time" => "Europe/London",
        "Russian Standard Time" => "Europe/Moscow",
        "Eastern Standard Time" => "America/New_York",
        "Central Standard Time" => "America/Chicago",
        "Mountain Standard Time" => "America/Denver",
        "Pacific Standard Time" => "America/Los_Angeles",
        "AUS Eastern Standard Time" => "Australia/Sydney",
        "New Zealand Standard Time" => "Pacific/Auckland",
        _ => return None,
    };
    iana.parse().ok()
}

/// `PT1H30M`, `P1D`, `-PT15M`.
fn duration(value: &str) -> Option<Duration> {
    let value = value.trim();
    let (sign, value) = match value.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.trim_start_matches('+')),
    };
    let value = value.strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut number = String::new();
    let mut in_time = false;
    for c in value.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            unit => {
                let amount: i64 = number.parse().ok()?;
                number.clear();
                total += match (unit, in_time) {
                    ('W', _) => Duration::weeks(amount),
                    ('D', _) => Duration::days(amount),
                    ('H', true) => Duration::hours(amount),
                    ('M', true) => Duration::minutes(amount),
                    ('S', true) => Duration::seconds(amount),
                    _ => return None,
                };
            }
        }
    }
    Some(total * sign)
}

/// Every occurrence in the feed that overlaps `from..to`, earliest first.
pub fn occurrences(source: &str, from: DateTime<Local>, to: DateTime<Local>) -> Vec<Occurrence> {
    let mut events: Vec<Event> = Vec::new();
    let mut calendar = None;
    let mut current: Option<Event> = None;
    let mut depth = 0;
    for line in unfold(source) {
        let Some((name, parameters, value)) = split(&line) else {
            continue;
        };
        match (name.as_str(), value.as_str()) {
            ("BEGIN", "VEVENT") => current = Some(Event::default()),
            ("END", "VEVENT") => events.extend(current.take()),
            // Alarms inside an event have their own description.
            ("BEGIN", _) if current.is_some() => depth += 1,
            ("END", _) if current.is_some() => depth -= 1,
            ("X-WR-CALNAME", _) if current.is_none() => calendar = Some(unescape(&value)),
            _ => {}
        }
        let Some(event) = current.as_mut().filter(|_| depth == 0) else {
            continue;
        };
        match name.as_str() {
            "UID" => event.uid = value,
            "SUMMARY" => event.title = unescape(&value),
            "DTSTART" => event.start = moment(&value, &parameters),
            "DTEND" => event.end = moment(&value, &parameters),
            "DURATION" => event.duration = duration(&value),
            "LOCATION" => event.location = Some(unescape(&value)).filter(|l| !l.is_empty()),
            "DESCRIPTION" => event.description = Some(unescape(&value)).filter(|d| !d.is_empty()),
            "URL" => event.url = Some(value),
            "RRULE" => event.rule = Some(value),
            "EXDATE" => event.exceptions.extend(
                value
                    .split(',')
                    .filter_map(|value| moment(value, &parameters)),
            ),
            "RECURRENCE-ID" => event.recurrence_id = moment(&value, &parameters),
            "STATUS" => event.cancelled = value.eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }

    // Occurrences moved or cancelled on their own, by series and start.
    let overrides: Vec<(String, DateTime<Local>)> = events
        .iter()
        .filter_map(|event| Some((event.uid.clone(), event.recurrence_id?.local())))
        .collect();

    let mut found = Vec::new();
    for event in &events {
        let Some(start) = event.start else {
            continue;
        };
        let all_day = matches!(start, Moment::Date(_));
        let length = match (event.end, event.duration) {
            (Some(end), _) => end.local() - start.local(),
            (None, Some(duration)) => duration,
            (None, None) if all_day => Duration::days(1),
            (None, None) => Duration::zero(),
        };
        let starts: Vec<DateTime<Local>> = match (&event.rule, event.recurrence_id) {
            (Some(rule), None) => expand(rule, start, from, to)
                .into_iter()
                .filter(|start| {
                    !event
                        .exceptions
                        .iter()
                        .any(|except| except.local() == *start)
                        && !overrides
                            .iter()
                            .any(|(uid, moved)| *uid == event.uid && moved == start)
                })
                .collect(),
            _ => vec![start.local()],
        };
        if event.cancelled {
            continue;
        }
        for start in starts {
            let end = start + length;
            if end > from && start < to || (length.is_zero() && start >= from && start < to) {
                found.push(Occurrence {
                    uid: event.uid.clone(),
                    title: match event.title.is_empty() {
                        true => "Untitled Event".to_owned(),
                        false => event.title.clone(),
                    },
                    start,
                    end,
                    all_day,
                    location: event.location.clone(),
                    description: event.description.clone(),
                    url: event.url.clone(),
                    calendar: calendar.clone(),
                });
            }
        }
    }
    found.sort_by(|a, b| a.start.cmp(&b.start).then(a.title.cmp(&b.title)));
    found
}

/// The rule parts [`expand`] understands; a rule with others is shown only
/// at its first occurrence rather than guessed at.
const UNDERSTOOD: [&str; 10] = [
    "FREQ",
    "INTERVAL",
    "COUNT",
    "UNTIL",
    "BYDAY",
    "BYMONTHDAY",
    "BYMONTH",
    "BYSETPOS",
    "WKST",
    "X-",
];

/// The starts of a recurring event from `from` up to `to`, from its `RRULE`.
/// Dates are counted in the zone the event was written in, so a 09:00
/// meeting in New York stays at 09:00 there across clock changes.
fn expand(
    rule: &str,
    start: Moment,
    from: DateTime<Local>,
    to: DateTime<Local>,
) -> Vec<DateTime<Local>> {
    let parts: HashMap<String, String> = rule
        .split(';')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.to_ascii_uppercase(), value.to_ascii_uppercase()))
        .collect();
    let frequency = parts.get("FREQ").map(String::as_str).unwrap_or("");
    let understood = matches!(frequency, "DAILY" | "WEEKLY" | "MONTHLY" | "YEARLY")
        && parts.keys().all(|key| {
            UNDERSTOOD
                .iter()
                .any(|known| key == known || (*known == "X-" && key.starts_with("X-")))
        });
    if !understood {
        return vec![start.local()];
    }
    let interval: i64 = parts
        .get("INTERVAL")
        .and_then(|interval| interval.parse().ok())
        .filter(|interval| *interval > 0)
        .unwrap_or(1);
    let count: Option<usize> = parts.get("COUNT").and_then(|count| count.parse().ok());
    let until = parts
        .get("UNTIL")
        .and_then(|until| moment(until, &HashMap::new()))
        .map(|until| match until {
            // An all-day UNTIL includes that whole day.
            Moment::Date(date) => local_midnight(date) + Duration::days(1),
            Moment::Time(..) => until.local(),
        });
    let week_start = parts
        .get("WKST")
        .and_then(|day| by_day(day))
        .map_or(Weekday::Mon, |(_, day)| day);
    let by_day: Vec<(Option<i64>, Weekday)> = parts
        .get("BYDAY")
        .map(|days| days.split(',').filter_map(by_day).collect())
        .unwrap_or_default();
    let by_month_day: Vec<i64> = parts
        .get("BYMONTHDAY")
        .map(|days| days.split(',').filter_map(|day| day.parse().ok()).collect())
        .unwrap_or_default();
    let by_month: Vec<u32> = parts
        .get("BYMONTH")
        .map(|months| {
            months
                .split(',')
                .filter_map(|month| month.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let by_set_position: Vec<i64> = parts
        .get("BYSETPOS")
        .map(|positions| {
            positions
                .split(',')
                .filter_map(|position| position.parse().ok())
                .collect()
        })
        .unwrap_or_default();

    // The first start's date and wall-clock time, in the event's own zone.
    let (date, time, zone) = match start {
        Moment::Date(date) => (date, None, Zone::Floating),
        Moment::Time(time, zone) => (time.date(), Some(time.time()), zone),
    };
    let at = |date: NaiveDate| match time {
        None => local_midnight(date),
        Some(time) => zone.resolve(date.and_time(time)),
    };
    // The days of `weekday`s in the week that holds `day`, with weeks
    // starting on `WKST`.
    let week_of = |day: NaiveDate| {
        let offset =
            (day.weekday().num_days_from_monday() + 7 - week_start.num_days_from_monday()) % 7;
        day - Duration::days(offset as i64)
    };
    let in_week = |weekday: Weekday| {
        (weekday.num_days_from_monday() + 7 - week_start.num_days_from_monday()) % 7
    };
    let days_of_month = |month: NaiveDate| -> Vec<NaiveDate> {
        match (by_day.is_empty(), by_month_day.is_empty()) {
            (true, true) => month.with_day(date.day()).into_iter().collect(),
            (_, false) => by_month_day
                .iter()
                .filter_map(|day| month_day(month, *day))
                .filter(|day| {
                    by_day.is_empty() || by_day.iter().any(|(_, weekday)| day.weekday() == *weekday)
                })
                .collect(),
            (false, true) => by_day
                .iter()
                .flat_map(|(ordinal, weekday)| weekdays_in_month(month, *weekday, *ordinal))
                .collect(),
        }
    };

    let limit = until.map_or(to, |until| until.min(to));
    // Without a count, the periods before `from` produce nothing shown and
    // are skipped, so a daily series from years ago costs nothing.
    let first_period = match count {
        Some(_) => 0,
        None => {
            let days = (from.date_naive() - date).num_days().max(0);
            let periods = match frequency {
                "DAILY" => days,
                "WEEKLY" => days / 7,
                "MONTHLY" => days / 31,
                _ => days / 366,
            };
            (periods / interval - 1).max(0)
        }
    };
    let mut starts = Vec::new();
    let mut produced = 0usize;
    // A malformed rule cannot run forever.
    for period in first_period..first_period + 5000 {
        let mut dates: Vec<NaiveDate> = match frequency {
            "DAILY" => vec![date + Duration::days(period * interval)],
            "WEEKLY" => {
                let week = week_of(date) + Duration::weeks(period * interval);
                match by_day.is_empty() {
                    true => vec![date + Duration::weeks(period * interval)],
                    false => by_day
                        .iter()
                        .map(|(_, weekday)| week + Duration::days(in_week(*weekday) as i64))
                        .collect(),
                }
            }
            "MONTHLY" => {
                let Some(month) = add_months(date.with_day(1).unwrap_or(date), period * interval)
                else {
                    break;
                };
                days_of_month(month)
            }
            _ => {
                let year = date.year() + (period * interval) as i32;
                let months: Vec<u32> = match by_month.is_empty() {
                    true => vec![date.month()],
                    false => by_month.clone(),
                };
                months
                    .into_iter()
                    .filter_map(|month| NaiveDate::from_ymd_opt(year, month, 1))
                    .flat_map(days_of_month)
                    .collect()
            }
        };
        if !by_month.is_empty() && frequency != "YEARLY" {
            dates.retain(|day| by_month.contains(&day.month()));
        }
        dates.sort();
        dates.dedup();
        if !by_set_position.is_empty() {
            let length = dates.len() as i64;
            let mut chosen: Vec<NaiveDate> = by_set_position
                .iter()
                .filter_map(|position| match *position {
                    position if position > 0 => dates.get(position as usize - 1).copied(),
                    position if position < 0 && -position <= length => {
                        dates.get((length + position) as usize).copied()
                    }
                    _ => None,
                })
                .collect();
            chosen.sort();
            chosen.dedup();
            dates = chosen;
        }
        for candidate in dates {
            if candidate < date {
                continue;
            }
            let start = at(candidate);
            if start > limit || count.is_some_and(|count| produced >= count) {
                return starts;
            }
            produced += 1;
            starts.push(start);
        }
    }
    starts
}

/// `MO`, `2TU`, `-1FR`.
fn by_day(value: &str) -> Option<(Option<i64>, Weekday)> {
    let value = value.trim();
    let (ordinal, day) = value.split_at(value.len().checked_sub(2)?);
    let weekday = match day {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    };
    let ordinal = match ordinal {
        "" | "+" => None,
        ordinal => Some(ordinal.trim_start_matches('+').parse().ok()?),
    };
    Some((ordinal, weekday))
}

fn add_months(date: NaiveDate, months: i64) -> Option<NaiveDate> {
    let total = date.year() as i64 * 12 + date.month0() as i64 + months;
    NaiveDate::from_ymd_opt((total / 12) as i32, (total % 12) as u32 + 1, 1)
}

fn days_in_month(month: NaiveDate) -> u32 {
    add_months(month, 1)
        .map(|next| (next - Duration::days(1)).day())
        .unwrap_or(28)
}

/// Day `day` of `month`; negative counts from the end.
fn month_day(month: NaiveDate, day: i64) -> Option<NaiveDate> {
    let days = days_in_month(month) as i64;
    let day = match day {
        day if day > 0 => day,
        day if day < 0 => days + day + 1,
        _ => return None,
    };
    (1..=days)
        .contains(&day)
        .then(|| month.with_day(day as u32))
        .flatten()
}

/// The `weekday`s of `month`, or only the `ordinal`th (the last for -1).
fn weekdays_in_month(month: NaiveDate, weekday: Weekday, ordinal: Option<i64>) -> Vec<NaiveDate> {
    let all: Vec<NaiveDate> = (1..=days_in_month(month))
        .filter_map(|day| month.with_day(day))
        .filter(|date| date.weekday() == weekday)
        .collect();
    match ordinal {
        None => all,
        Some(ordinal) if ordinal > 0 => {
            all.get(ordinal as usize - 1).copied().into_iter().collect()
        }
        Some(ordinal) => all
            .len()
            .checked_sub(ordinal.unsigned_abs() as usize)
            .and_then(|index| all.get(index).copied())
            .into_iter()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(text: &str) -> DateTime<Local> {
        Local
            .from_local_datetime(&NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap())
            .unwrap()
    }

    const FEED: &str = "BEGIN:VCALENDAR\r\n\
X-WR-CALNAME:Work\r\n\
BEGIN:VEVENT\r\n\
UID:standup\r\n\
SUMMARY:Stand-up\r\n\
DTSTART:20240101T093000\r\n\
DTEND:20240101T094500\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=6\r\n\
EXDATE:20240103T093000\r\n\
LOCATION:https://meet.google.com/abc-defg-hij\r\n\
BEGIN:VALARM\r\n\
DESCRIPTION:Reminder\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:standup\r\n\
RECURRENCE-ID:20240105T093000\r\n\
SUMMARY:Stand-up (moved)\r\n\
DTSTART:20240105T110000\r\n\
DTEND:20240105T111500\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:holiday\r\n\
SUMMARY:New Year\\, again\r\n\
DTSTART;VALUE=DATE:20240101\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:review\r\n\
SUMMARY:Review\r\n\
DTSTART:20240131T150000\r\n\
DURATION:PT1H\r\n\
RRULE:FREQ=MONTHLY;BYDAY=-1WE\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn test_occurrences_expand_rules_exceptions_and_overrides() {
        let found = occurrences(FEED, local("2024-01-01 00:00"), local("2024-03-01 00:00"));
        let titles: Vec<(String, String)> = found
            .iter()
            .map(|o| (o.start.format("%m-%d %H:%M").to_string(), o.title.clone()))
            .collect();
        assert_eq!(
            titles,
            [
                ("01-01 00:00", "New Year, again"),
                ("01-01 09:30", "Stand-up"),
                ("01-05 11:00", "Stand-up (moved)"),
                ("01-08 09:30", "Stand-up"),
                ("01-10 09:30", "Stand-up"),
                ("01-12 09:30", "Stand-up"),
                ("01-31 15:00", "Review"),
                ("02-28 15:00", "Review"),
            ]
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
        );
        assert!(found[0].all_day);
        assert_eq!(found[1].calendar.as_deref(), Some("Work"));
        assert_eq!(found[1].end - found[1].start, Duration::minutes(15));
        assert_eq!(found[6].end - found[6].start, Duration::hours(1));
    }

    fn utc_starts(feed: &str, from: &str, to: &str) -> Vec<String> {
        occurrences(feed, local(from), local(to))
            .iter()
            .map(|o| {
                o.start
                    .with_timezone(&Utc)
                    .format("%m-%d %H:%M")
                    .to_string()
            })
            .collect()
    }

    fn event(rule: &str, start: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nSUMMARY:x\r\n{start}\r\n\
             RRULE:{rule}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    }

    #[test]
    fn test_recurrences_keep_their_zone_and_rule_parts() {
        // 09:00 in New York, across the US change to daylight time.
        let feed = event(
            "FREQ=WEEKLY;COUNT=3",
            "DTSTART;TZID=America/New_York:20240304T090000",
        );
        assert_eq!(
            utc_starts(&feed, "2024-03-01 00:00", "2024-04-01 00:00"),
            ["03-04 14:00", "03-11 13:00", "03-18 13:00"]
        );
        // The last weekday of each month.
        let feed = event(
            "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=2",
            "DTSTART:20240131T100000Z",
        );
        assert_eq!(
            utc_starts(&feed, "2024-01-01 00:00", "2024-04-01 00:00"),
            ["01-31 10:00", "02-29 10:00"]
        );
        // Every other week, on Sunday and the Monday after it.
        let feed = event(
            "FREQ=WEEKLY;WKST=SU;INTERVAL=2;BYDAY=SU,MO;COUNT=4",
            "DTSTART:20240107T100000Z",
        );
        assert_eq!(
            utc_starts(&feed, "2024-01-01 00:00", "2024-03-01 00:00"),
            ["01-07 10:00", "01-08 10:00", "01-21 10:00", "01-22 10:00"]
        );
        // The second Sunday of May.
        let feed = event(
            "FREQ=YEARLY;BYMONTH=5;BYDAY=2SU",
            "DTSTART;VALUE=DATE:20240512",
        );
        let starts: Vec<String> =
            occurrences(&feed, local("2025-01-01 00:00"), local("2026-12-31 00:00"))
                .iter()
                .map(|o| o.start.format("%Y-%m-%d").to_string())
                .collect();
        assert_eq!(starts, ["2025-05-11", "2026-05-10"]);
        // A rule with parts not understood shows its first occurrence only.
        let feed = event("FREQ=YEARLY;BYWEEKNO=20", "DTSTART:20240101T100000Z");
        assert_eq!(
            utc_starts(&feed, "2024-01-01 00:00", "2026-01-01 00:00"),
            ["01-01 10:00"]
        );
        // A daily series from long ago still reaches today.
        let feed = event("FREQ=DAILY", "DTSTART:20000101T100000Z");
        assert_eq!(
            utc_starts(&feed, "2024-06-01 00:00", "2024-06-03 00:00").len(),
            2
        );
    }

    #[test]
    fn test_zones_and_durations() {
        let mut parameters = HashMap::new();
        parameters.insert("TZID".to_owned(), "China Standard Time".to_owned());
        let time = moment("20240101T090000", &parameters).unwrap().local();
        assert_eq!(
            time.with_timezone(&Utc).format("%H:%M").to_string(),
            "01:00"
        );
        assert_eq!(duration("PT1H30M"), Some(Duration::minutes(90)));
        assert_eq!(duration("P1W"), Some(Duration::weeks(1)));
        assert_eq!(duration("-PT15M"), Some(Duration::minutes(-15)));
        assert_eq!(by_day("-1FR"), Some((Some(-1), Weekday::Fri)));
        assert_eq!(unfold("A:1\r\n 2\r\nB:3").len(), 2);
    }
}
