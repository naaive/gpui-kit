//! Dates and times typed into the search field:
//!
//! - `time in tokyo`, `london time`, `now in America/Chicago`
//! - `5pm pst to taipei`, `9:30 in london` (from local time)
//! - `days until 2026-12-25`, `days since jan 1`, `days until christmas`
//! - `today + 30 days`, `now - 3 hours`, `tomorrow`

use std::str::FromStr as _;

use chrono::{
    DateTime, Datelike as _, Duration, Local, Months, NaiveDate, NaiveTime, TimeZone, Utc,
};
use chrono_tz::Tz;

use super::conversion::Conversion;

/// Cities and abbreviations people type, and their zones. Anything else is
/// tried as an IANA name such as `Europe/Berlin`.
const PLACES: &[(&str, &str)] = &[
    ("utc", "UTC"),
    ("gmt", "UTC"),
    ("london", "Europe/London"),
    ("uk", "Europe/London"),
    ("bst", "Europe/London"),
    ("paris", "Europe/Paris"),
    ("berlin", "Europe/Berlin"),
    ("madrid", "Europe/Madrid"),
    ("rome", "Europe/Rome"),
    ("amsterdam", "Europe/Amsterdam"),
    ("zurich", "Europe/Zurich"),
    ("stockholm", "Europe/Stockholm"),
    ("cet", "Europe/Paris"),
    ("cest", "Europe/Paris"),
    ("moscow", "Europe/Moscow"),
    ("istanbul", "Europe/Istanbul"),
    ("dubai", "Asia/Dubai"),
    ("india", "Asia/Kolkata"),
    ("mumbai", "Asia/Kolkata"),
    ("delhi", "Asia/Kolkata"),
    ("bangalore", "Asia/Kolkata"),
    ("ist", "Asia/Kolkata"),
    ("bangkok", "Asia/Bangkok"),
    ("singapore", "Asia/Singapore"),
    ("hong kong", "Asia/Hong_Kong"),
    ("hk", "Asia/Hong_Kong"),
    ("hkt", "Asia/Hong_Kong"),
    ("taipei", "Asia/Taipei"),
    ("taiwan", "Asia/Taipei"),
    ("beijing", "Asia/Shanghai"),
    ("shanghai", "Asia/Shanghai"),
    ("shenzhen", "Asia/Shanghai"),
    ("china", "Asia/Shanghai"),
    ("seoul", "Asia/Seoul"),
    ("korea", "Asia/Seoul"),
    ("kst", "Asia/Seoul"),
    ("tokyo", "Asia/Tokyo"),
    ("japan", "Asia/Tokyo"),
    ("jst", "Asia/Tokyo"),
    ("sydney", "Australia/Sydney"),
    ("melbourne", "Australia/Melbourne"),
    ("aest", "Australia/Sydney"),
    ("auckland", "Pacific/Auckland"),
    ("new york", "America/New_York"),
    ("nyc", "America/New_York"),
    ("boston", "America/New_York"),
    ("toronto", "America/Toronto"),
    ("est", "America/New_York"),
    ("edt", "America/New_York"),
    ("et", "America/New_York"),
    ("chicago", "America/Chicago"),
    ("cst", "America/Chicago"),
    ("cdt", "America/Chicago"),
    ("denver", "America/Denver"),
    ("mst", "America/Denver"),
    ("los angeles", "America/Los_Angeles"),
    ("la", "America/Los_Angeles"),
    ("san francisco", "America/Los_Angeles"),
    ("sf", "America/Los_Angeles"),
    ("seattle", "America/Los_Angeles"),
    ("vancouver", "America/Vancouver"),
    ("pst", "America/Los_Angeles"),
    ("pdt", "America/Los_Angeles"),
    ("pt", "America/Los_Angeles"),
    ("mexico city", "America/Mexico_City"),
    ("sao paulo", "America/Sao_Paulo"),
    ("são paulo", "America/Sao_Paulo"),
    ("buenos aires", "America/Argentina/Buenos_Aires"),
];

/// Places written in capitals in an answer.
const ABBREVIATIONS: &[&str] = &[
    "utc", "gmt", "uk", "bst", "cet", "cest", "ist", "hk", "hkt", "kst", "jst", "aest", "nyc",
    "est", "edt", "et", "cst", "cdt", "mst", "la", "sf", "pst", "pdt", "pt",
];

/// A zone, or the local one, with the name to show for it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Zone {
    Local,
    Named(Tz, &'static str),
}

fn zone(name: &str) -> Option<Zone> {
    let name = name.trim().trim_end_matches(" time").trim();
    if matches!(name, "local" | "here" | "my time") {
        return Some(Zone::Local);
    }
    if let Some((place, zone)) = PLACES.iter().find(|(place, _)| *place == name) {
        return Some(Zone::Named(Tz::from_str(zone).ok()?, place));
    }
    let tz = *chrono_tz::TZ_VARIANTS
        .iter()
        .find(|tz| tz.name().eq_ignore_ascii_case(name))?;
    Some(Zone::Named(tz, tz.name()))
}

/// How a zone reads in an answer: `Tokyo`, `New York`, `PST`, `Berlin`.
fn display_name(zone: Zone) -> String {
    match zone {
        Zone::Local => "local time".into(),
        Zone::Named(_, name) if ABBREVIATIONS.contains(&name) => name.to_uppercase(),
        Zone::Named(_, name) => name
            .rsplit('/')
            .next()
            .unwrap_or(name)
            .replace('_', " ")
            .split(' ')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Answers `query`, or `None` when it is not about dates or times.
pub fn answer(query: &str) -> Option<Conversion> {
    answer_at(query, Utc::now())
}

fn answer_at(query: &str, now: DateTime<Utc>) -> Option<Conversion> {
    let query = query.trim().to_lowercase();
    time_in(&query, now)
        .or_else(|| convert_time(&query, now))
        .or_else(|| days_between(&query, now))
        .or_else(|| shift(&query, now))
}

/// The time `now` in `zone`, as `14:05 · Wed, Oct 1`.
fn show(now: DateTime<Utc>, zone: Zone) -> (String, String) {
    let (time, day) = match zone {
        Zone::Local => {
            let local = now.with_timezone(&Local);
            (
                local.format("%H:%M").to_string(),
                local.format("%a, %b %-d").to_string(),
            )
        }
        Zone::Named(tz, _) => {
            let there = now.with_timezone(&tz);
            (
                there.format("%H:%M").to_string(),
                there.format("%a, %b %-d").to_string(),
            )
        }
    };
    let display = format!("{time} · {day} in {}", display_name(zone));
    (time, display)
}

fn time_in(query: &str, now: DateTime<Utc>) -> Option<Conversion> {
    let place = [
        "time in ",
        "now in ",
        "what time is it in ",
        "current time in ",
    ]
    .iter()
    .find_map(|prefix| query.strip_prefix(prefix))
    .or_else(|| query.strip_suffix(" time"))?;
    let zone = zone(place)?;
    let (value, display) = show(now, zone);
    Some(Conversion { value, display })
}

/// `5pm`, `5:30 pm`, `17:00`, `noon`, `midnight`.
fn parse_time(text: &str) -> Option<NaiveTime> {
    let text = text.trim().replace(' ', "");
    match text.as_str() {
        "noon" => return NaiveTime::from_hms_opt(12, 0, 0),
        "midnight" => return NaiveTime::from_hms_opt(0, 0, 0),
        _ => {}
    }
    let (clock, meridiem) = match (text.strip_suffix("am"), text.strip_suffix("pm")) {
        (Some(clock), _) => (clock, Some(false)),
        (_, Some(clock)) => (clock, Some(true)),
        _ => (text.as_str(), None),
    };
    let (hour, minute) = match clock.split_once(':') {
        Some((hour, minute)) => (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?),
        None if meridiem.is_some() => (clock.parse::<u32>().ok()?, 0),
        // A bare number is not a time: `5 to 7` is not a conversion.
        None => return None,
    };
    let hour = match meridiem {
        Some(_) if !(1..=12).contains(&hour) => return None,
        Some(true) => hour % 12 + 12,
        Some(false) => hour % 12,
        None => hour,
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// `5pm pst to taipei`, `9:30 in london` (from local time).
fn convert_time(query: &str, now: DateTime<Utc>) -> Option<Conversion> {
    let (source, target) = [" to ", " in "]
        .iter()
        .filter_map(|separator| query.rfind(separator).map(|ix| (ix, separator.len())))
        .max_by_key(|(ix, _)| *ix)
        .map(|(ix, length)| (&query[..ix], &query[ix + length..]))?;
    let target = zone(target)?;
    // The time is the first word or two; the rest names its zone.
    let words: Vec<&str> = source.split_whitespace().collect();
    let (time, from) = (1..=words.len().min(2)).rev().find_map(|count| {
        let time = parse_time(&words[..count].join(" "))?;
        let rest = words[count..].join(" ");
        let from = match rest.is_empty() {
            true => Zone::Local,
            false => zone(&rest)?,
        };
        Some((time, from))
    })?;
    let instant: DateTime<Utc> = match from {
        Zone::Local => {
            let date = now.with_timezone(&Local).date_naive();
            Local
                .from_local_datetime(&date.and_time(time))
                .earliest()?
                .with_timezone(&Utc)
        }
        Zone::Named(tz, _) => {
            let date = now.with_timezone(&tz).date_naive();
            tz.from_local_datetime(&date.and_time(time))
                .earliest()?
                .with_timezone(&Utc)
        }
    };
    let (value, display) = show(instant, target);
    Some(Conversion { value, display })
}

/// `2026-12-25`, `dec 25`, `25 dec`, `december 25 2027`, `christmas`.
fn parse_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let text = text.trim().trim_end_matches('?').trim();
    match text {
        "today" => return Some(today),
        "tomorrow" => return today.succ_opt(),
        "yesterday" => return today.pred_opt(),
        "christmas" | "xmas" => return next_annual(today, 12, 25),
        "new year" | "new years" | "new year's" => return next_annual(today, 1, 1),
        _ => {}
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(date);
    }
    let words: Vec<&str> = text.split([' ', ',']).filter(|w| !w.is_empty()).collect();
    let (month, day, year) = match words.as_slice() {
        [first, second] | [first, second, _] => match (month(first), month(second)) {
            (Some(month), _) => (month, second.parse::<u32>().ok()?, words.get(2)),
            (_, Some(month)) => (month, first.parse::<u32>().ok()?, words.get(2)),
            _ => return None,
        },
        _ => return None,
    };
    match year {
        Some(year) => NaiveDate::from_ymd_opt(year.parse().ok()?, month, day),
        // Without a year, the next one.
        None => next_annual(today, month, day),
    }
}

fn next_annual(today: NaiveDate, month: u32, day: u32) -> Option<NaiveDate> {
    let this_year = NaiveDate::from_ymd_opt(today.year(), month, day)?;
    match this_year >= today {
        true => Some(this_year),
        false => NaiveDate::from_ymd_opt(today.year() + 1, month, day),
    }
}

fn month(word: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let word = word.trim_end_matches('.');
    (word.len() >= 3)
        .then(|| MONTHS.iter().position(|month| word.starts_with(month)))
        .flatten()
        .map(|ix| ix as u32 + 1)
}

fn days_between(query: &str, now: DateTime<Utc>) -> Option<Conversion> {
    let today = now.with_timezone(&Local).date_naive();
    let (date, until) = if let Some(rest) = query.strip_prefix("days until ") {
        (parse_date(rest, today)?, true)
    } else if let Some(rest) = query.strip_prefix("days since ") {
        (parse_date(rest, today)?, false)
    } else {
        return None;
    };
    let days = match until {
        true => (date - today).num_days(),
        false => (today - date).num_days(),
    };
    let weekday = date.format("%A, %B %-d, %Y");
    Some(Conversion {
        value: days.to_string(),
        display: format!(
            "{days} {} {} {weekday}",
            if days.abs() == 1 { "day" } else { "days" },
            if until { "until" } else { "since" }
        ),
    })
}

/// `today + 30 days`, `now - 3 hours`, `tomorrow`.
fn shift(query: &str, now: DateTime<Utc>) -> Option<Conversion> {
    let local = now.with_timezone(&Local);
    let (base, rest) = ["today", "now", "tomorrow", "yesterday"]
        .iter()
        .find_map(|base| query.strip_prefix(base).map(|rest| (*base, rest.trim())))?;
    let start = match base {
        "tomorrow" => local + Duration::days(1),
        "yesterday" => local - Duration::days(1),
        _ => local,
    };
    let with_time = base == "now";
    if rest.is_empty() {
        // `today` alone is a search for commands, not a question.
        if base == "today" || base == "now" {
            return None;
        }
        return Some(date_answer(start, with_time));
    }
    let (sign, rest) = match rest.split_at_checked(1)? {
        ("+", rest) => (1i64, rest.trim()),
        ("-", rest) => (-1i64, rest.trim()),
        _ => return None,
    };
    let (amount, unit) = rest.split_once(' ').unwrap_or((rest, "days"));
    let amount: i64 = amount.parse().ok()?;
    let amount = amount * sign;
    let unit = unit.trim().trim_end_matches('s');
    // Every step is checked: an amount past the calendar's range is no
    // answer, not a crash.
    let months = |count: i64| -> Option<DateTime<Local>> {
        let months = Months::new(u32::try_from(count.unsigned_abs()).ok()?);
        match count >= 0 {
            true => start.checked_add_months(months),
            false => start.checked_sub_months(months),
        }
    };
    let shifted = match unit {
        "minute" | "min" => start.checked_add_signed(Duration::try_minutes(amount)?)?,
        "hour" | "hr" | "h" => start.checked_add_signed(Duration::try_hours(amount)?)?,
        "day" | "d" => start.checked_add_signed(Duration::try_days(amount)?)?,
        "week" | "wk" | "w" => start.checked_add_signed(Duration::try_weeks(amount)?)?,
        "month" | "mo" => months(amount)?,
        "year" | "yr" | "y" => months(amount.checked_mul(12)?)?,
        _ => return None,
    };
    let with_time = with_time || matches!(unit, "minute" | "min" | "hour" | "hr" | "h");
    Some(date_answer(shifted, with_time))
}

fn date_answer(date: DateTime<Local>, with_time: bool) -> Conversion {
    match with_time {
        true => Conversion {
            value: date.format("%Y-%m-%d %H:%M").to_string(),
            display: date.format("%A, %B %-d, %Y at %H:%M").to_string(),
        },
        false => Conversion {
            value: date.format("%Y-%m-%d").to_string(),
            display: date.format("%A, %B %-d, %Y").to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn display(query: &str) -> Option<String> {
        answer_at(query, at("2026-10-01T06:00:00Z")).map(|answer| answer.display)
    }

    #[test]
    fn test_time_in_places() {
        assert_eq!(
            display("time in Tokyo").as_deref(),
            Some("15:00 · Thu, Oct 1 in Tokyo")
        );
        assert_eq!(
            display("new york time").as_deref(),
            Some("02:00 · Thu, Oct 1 in New York")
        );
        assert_eq!(
            display("now in europe/berlin").as_deref(),
            Some("08:00 · Thu, Oct 1 in Berlin")
        );
        assert_eq!(display("time in nowhere"), None);
        assert!(display("time in rome").unwrap().ends_with("in Rome"));
    }

    #[test]
    fn test_converts_times_between_zones() {
        // It is still September 30 in Los Angeles at this instant.
        assert_eq!(
            display("5pm pst to taipei").as_deref(),
            Some("08:00 · Thu, Oct 1 in Taipei")
        );
        assert_eq!(
            display("9:30 utc in jst").as_deref(),
            Some("18:30 · Thu, Oct 1 in JST")
        );
        assert_eq!(display("5 to 7"), None);
        assert_eq!(display("go to london"), None);
    }

    #[test]
    fn test_days_and_shifts() {
        let today = at("2026-10-01T06:00:00Z")
            .with_timezone(&Local)
            .date_naive();
        assert_eq!(
            parse_date("dec 25", today),
            NaiveDate::from_ymd_opt(2026, 12, 25)
        );
        assert_eq!(
            parse_date("1 jan", today),
            NaiveDate::from_ymd_opt(2027, 1, 1)
        );
        assert_eq!(
            parse_date("2027-03-04", today),
            NaiveDate::from_ymd_opt(2027, 3, 4)
        );
        let until = answer_at("days until christmas", at("2026-10-01T06:00:00Z")).unwrap();
        assert_eq!(
            until.value,
            (NaiveDate::from_ymd_opt(2026, 12, 25).unwrap() - today)
                .num_days()
                .to_string()
        );
        assert!(display("today + 30 days").is_some());
        assert!(display("tomorrow").is_some());
        assert_eq!(display("today"), None, "a lone word still searches");
        assert_eq!(display("today + 3 parsecs"), None);
        for huge in [
            "today + 100000000",
            "now - 9999999999 weeks",
            "today + 9999999999 years",
        ] {
            assert_eq!(display(huge), None, "{huge}");
        }
    }
}
