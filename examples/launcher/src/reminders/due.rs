//! When a reminder is due, as people type it: `tomorrow 9am`, `in 2 hours`,
//! `fri 17:00`, `2026-10-03 14:30`, `明天下午3点`, `30分钟后`.

use std::sync::OnceLock;

use chrono::{
    Datelike as _, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike as _, Weekday,
};
use regex::Regex;

/// The time of day a reminder with only a date is due.
const DEFAULT_HOUR: u32 = 9;
/// The time of day `tonight` means.
const EVENING_HOUR: u32 = 20;

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("a valid pattern"))
}

/// `in 2 hours`, `2h`, `30 min later`, `30分钟后`.
fn relative(text: &str) -> Option<Duration> {
    static ENGLISH: OnceLock<Regex> = OnceLock::new();
    static CHINESE: OnceLock<Regex> = OnceLock::new();
    let english = regex(
        &ENGLISH,
        r"^(?:in\s+)?(\d+)\s*(m|min|mins|minutes?|h|hr|hrs|hours?|d|days?|w|wk|weeks?)(?:\s+later)?$",
    );
    let chinese = regex(
        &CHINESE,
        r"^(\d+)\s*(分钟|分|小时|个小时|天|周|星期|个星期)后?$",
    );
    let (count, unit) = match english.captures(text).or_else(|| chinese.captures(text)) {
        Some(captures) => (captures[1].parse::<i64>().ok()?, captures[2].to_owned()),
        None => return None,
    };
    Some(match unit.as_str() {
        "m" | "min" | "mins" | "minute" | "minutes" | "分钟" | "分" => Duration::minutes(count),
        "h" | "hr" | "hrs" | "hour" | "hours" | "小时" | "个小时" => Duration::hours(count),
        "d" | "day" | "days" | "天" => Duration::days(count),
        _ => Duration::weeks(count),
    })
}

fn weekday(name: &str) -> Option<Weekday> {
    Some(match name {
        "mon" | "monday" | "一" => Weekday::Mon,
        "tue" | "tues" | "tuesday" | "二" => Weekday::Tue,
        "wed" | "wednesday" | "三" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" | "四" => Weekday::Thu,
        "fri" | "friday" | "五" => Weekday::Fri,
        "sat" | "saturday" | "六" => Weekday::Sat,
        "sun" | "sunday" | "日" | "天" => Weekday::Sun,
        _ => return None,
    })
}

fn month(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    MONTHS
        .iter()
        .position(|month| name.starts_with(month))
        .map(|index| index as u32 + 1)
}

/// The next `month`/`day` from `today`, this year or the next.
fn upcoming(today: NaiveDate, month: u32, day: u32) -> Option<NaiveDate> {
    let this_year = NaiveDate::from_ymd_opt(today.year(), month, day)?;
    match this_year < today {
        true => NaiveDate::from_ymd_opt(today.year() + 1, month, day),
        false => Some(this_year),
    }
}

/// Finds a date in `text`, returning it, whether it was `tonight`, and the
/// text with the date taken out.
fn take_date(text: &str, today: NaiveDate) -> Option<(NaiveDate, bool, String)> {
    static ISO: OnceLock<Regex> = OnceLock::new();
    static SLASH: OnceLock<Regex> = OnceLock::new();
    static CHINESE: OnceLock<Regex> = OnceLock::new();
    static NAMED_MONTH: OnceLock<Regex> = OnceLock::new();
    static DAY_MONTH: OnceLock<Regex> = OnceLock::new();
    static WEEKDAY: OnceLock<Regex> = OnceLock::new();
    static CHINESE_WEEKDAY: OnceLock<Regex> = OnceLock::new();
    let take =
        |range: std::ops::Range<usize>| format!("{} {}", &text[..range.start], &text[range.end..]);

    if let Some(found) = regex(&ISO, r"\b(\d{4})-(\d{1,2})-(\d{1,2})\b").captures(text) {
        let date = NaiveDate::from_ymd_opt(
            found[1].parse().ok()?,
            found[2].parse().ok()?,
            found[3].parse().ok()?,
        )?;
        return Some((date, false, take(found.get(0)?.range())));
    }
    if let Some(found) = regex(&SLASH, r"\b(\d{1,2})/(\d{1,2})\b").captures(text) {
        let date = upcoming(today, found[1].parse().ok()?, found[2].parse().ok()?)?;
        return Some((date, false, take(found.get(0)?.range())));
    }
    if let Some(found) = regex(&CHINESE, r"(\d{1,2})月(\d{1,2})[日号]?").captures(text) {
        let date = upcoming(today, found[1].parse().ok()?, found[2].parse().ok()?)?;
        return Some((date, false, take(found.get(0)?.range())));
    }
    if let Some(found) = regex(
        &NAMED_MONTH,
        r"\b(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+(\d{1,2})(?:st|nd|rd|th)?\b",
    )
    .captures(text)
    {
        let date = upcoming(today, month(&found[1])?, found[2].parse().ok()?)?;
        return Some((date, false, take(found.get(0)?.range())));
    }
    if let Some(found) = regex(
        &DAY_MONTH,
        r"\b(\d{1,2})(?:st|nd|rd|th)?\s+(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\b",
    )
    .captures(text)
    {
        let date = upcoming(today, month(&found[2])?, found[1].parse().ok()?)?;
        return Some((date, false, take(found.get(0)?.range())));
    }
    // Longest words first, so `day after tomorrow` is not read as `tomorrow`.
    let words: [(&str, i64, bool); 9] = [
        ("day after tomorrow", 2, false),
        ("tomorrow", 1, false),
        ("tonight", 0, true),
        ("today", 0, false),
        ("大后天", 3, false),
        ("后天", 2, false),
        ("明天", 1, false),
        ("今晚", 0, true),
        ("今天", 0, false),
    ];
    for (word, days, evening) in words {
        if let Some(start) = text.find(word) {
            return Some((
                today + Duration::days(days),
                evening,
                take(start..start + word.len()),
            ));
        }
    }
    let on_weekday = |day: Weekday, next: bool| {
        let ahead = (day.num_days_from_monday() + 7 - today.weekday().num_days_from_monday()) % 7;
        // `next friday` on a Friday is a week away; `friday` is today.
        let ahead = match (ahead, next) {
            (0, true) => 7,
            (ahead, _) => ahead,
        };
        today + Duration::days(ahead as i64)
    };
    if let Some(found) = regex(
        &WEEKDAY,
        r"\b(next\s+)?(monday|tuesday|wednesday|thursday|friday|saturday|sunday|mon|tues|tue|wed|thurs|thur|thu|fri|sat|sun)\b",
    )
    .captures(text)
    {
        let date = on_weekday(weekday(&found[2])?, found.get(1).is_some());
        return Some((date, false, take(found.get(0)?.range())));
    }
    if let Some(found) = regex(
        &CHINESE_WEEKDAY,
        r"(下)?(?:周|星期|礼拜)([一二三四五六日天])",
    )
    .captures(text)
    {
        let mut date = on_weekday(weekday(&found[2])?, false);
        if found.get(1).is_some() {
            // 下周五: Friday of next week.
            let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
            date = monday
                + Duration::weeks(1)
                + Duration::days(weekday(&found[2])?.num_days_from_monday() as i64);
        }
        return Some((date, false, take(found.get(0)?.range())));
    }
    None
}

/// Finds a time of day in `text`: `9am`, `9:30 pm`, `14:00`, `noon`,
/// `下午3点半`, `9点30分`.
fn take_time(text: &str) -> Option<NaiveTime> {
    static ENGLISH: OnceLock<Regex> = OnceLock::new();
    static CHINESE: OnceLock<Regex> = OnceLock::new();
    let text = text.trim();
    if text.contains("noon") || text.contains("中午12点") {
        return NaiveTime::from_hms_opt(12, 0, 0);
    }
    if text.contains("midnight") {
        return NaiveTime::from_hms_opt(23, 59, 0);
    }
    if let Some(found) = regex(
        &CHINESE,
        r"(凌晨|早上|上午|中午|下午|傍晚|晚上)?\s*(\d{1,2})\s*[点时]\s*(半|\d{1,2})?\s*分?",
    )
    .captures(text)
    {
        let mut hour: u32 = found[2].parse().ok()?;
        let minute = match found.get(3).map(|minute| minute.as_str()) {
            Some("半") => 30,
            Some(minute) => minute.parse().ok()?,
            None => 0,
        };
        if matches!(
            found.get(1).map(|part| part.as_str()),
            Some("下午" | "傍晚" | "晚上")
        ) && hour < 12
        {
            hour += 12;
        }
        if found.get(1).is_some_and(|part| part.as_str() == "中午") && hour < 6 {
            hour += 12;
        }
        return NaiveTime::from_hms_opt(hour, minute, 0);
    }
    let found = regex(
        &ENGLISH,
        r"(?:\bat\s+)?\b(\d{1,2})(?::(\d{2}))?\s*(am|pm|a\.m\.|p\.m\.)?\b",
    )
    .captures(text)?;
    let mut hour: u32 = found[1].parse().ok()?;
    let minute: u32 = match found.get(2) {
        Some(minute) => minute.as_str().parse().ok()?,
        None => 0,
    };
    // A lone number is a time only with am/pm or minutes.
    let meridiem = found
        .get(3)
        .map(|meridiem| meridiem.as_str().starts_with('p'));
    if meridiem.is_none() && found.get(2).is_none() {
        return None;
    }
    match meridiem {
        Some(true) if hour < 12 => hour += 12,
        Some(false) if hour == 12 => hour = 0,
        _ => {}
    }
    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// When `text` says a reminder is due, from `now`; `None` when it says
/// nothing that reads as a time.
pub fn parse(text: &str, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }
    if let Some(after) = relative(&text) {
        return Some(now + after);
    }
    let today = now.date();
    match take_date(&text, today) {
        Some((date, evening, rest)) => {
            let rest = rest.trim();
            let time = match rest.is_empty() {
                true => None,
                false => Some(take_time(rest)?),
            };
            let time = match (time, evening) {
                (None, true) => NaiveTime::from_hms_opt(EVENING_HOUR, 0, 0)?,
                (None, false) => NaiveTime::from_hms_opt(DEFAULT_HOUR, 0, 0)?,
                // `tonight at 8` is in the evening.
                (Some(time), true) if time.hour() < 12 => time + Duration::hours(12),
                (Some(time), _) => time,
            };
            Some(date.and_time(time))
        }
        None => {
            // A time alone is today's, or tomorrow's once it has passed.
            let time = take_time(&text)?;
            let today_at = today.and_time(time);
            Some(match today_at > now {
                true => today_at,
                false => today_at + Duration::days(1),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Option<String> {
        // A Wednesday afternoon.
        let now = NaiveDate::from_ymd_opt(2026, 9, 30)
            .unwrap()
            .and_hms_opt(15, 0, 0)
            .unwrap();
        parse(text, now).map(|due| due.format("%Y-%m-%d %H:%M").to_string())
    }

    #[test]
    fn test_reads_english_times() {
        assert_eq!(at("in 2 hours").as_deref(), Some("2026-09-30 17:00"));
        assert_eq!(at("30m").as_deref(), Some("2026-09-30 15:30"));
        assert_eq!(at("tomorrow").as_deref(), Some("2026-10-01 09:00"));
        assert_eq!(at("tomorrow 2:30pm").as_deref(), Some("2026-10-01 14:30"));
        assert_eq!(at("tonight").as_deref(), Some("2026-09-30 20:00"));
        assert_eq!(at("fri 17:00").as_deref(), Some("2026-10-02 17:00"));
        assert_eq!(at("wednesday").as_deref(), Some("2026-09-30 09:00"));
        assert_eq!(at("next wed").as_deref(), Some("2026-10-07 09:00"));
        assert_eq!(at("2026-10-03 14:30").as_deref(), Some("2026-10-03 14:30"));
        assert_eq!(at("oct 5 at 8am").as_deref(), Some("2026-10-05 08:00"));
        assert_eq!(at("3 jan").as_deref(), Some("2027-01-03 09:00"));
        assert_eq!(at("10/1").as_deref(), Some("2026-10-01 09:00"));
        // A time alone: today, or tomorrow once it has passed.
        assert_eq!(at("6pm").as_deref(), Some("2026-09-30 18:00"));
        assert_eq!(at("9:15").as_deref(), Some("2026-10-01 09:15"));
        assert_eq!(at("noon").as_deref(), Some("2026-10-01 12:00"));
    }

    #[test]
    fn test_reads_chinese_times() {
        assert_eq!(at("30分钟后").as_deref(), Some("2026-09-30 15:30"));
        assert_eq!(at("明天").as_deref(), Some("2026-10-01 09:00"));
        assert_eq!(at("明天下午3点").as_deref(), Some("2026-10-01 15:00"));
        assert_eq!(at("后天上午10点半").as_deref(), Some("2026-10-02 10:30"));
        assert_eq!(at("今晚8点").as_deref(), Some("2026-09-30 20:00"));
        assert_eq!(at("周五 9点30分").as_deref(), Some("2026-10-02 09:30"));
        assert_eq!(at("下周一").as_deref(), Some("2026-10-05 09:00"));
        assert_eq!(at("10月3日 14:00").as_deref(), Some("2026-10-03 14:00"));
        assert_eq!(at("晚上7点").as_deref(), Some("2026-09-30 19:00"));
        assert_eq!(at("tonight at 8pm").as_deref(), Some("2026-09-30 20:00"));
        assert_eq!(at("tonight 9:30").as_deref(), Some("2026-09-30 21:30"));
    }

    #[test]
    fn test_rejects_what_is_not_a_time() {
        assert_eq!(at(""), None);
        assert_eq!(at("buy milk"), None);
        assert_eq!(at("42"), None);
        assert_eq!(at("tomorrow buy milk"), None);
        assert_eq!(at("2026-02-30"), None);
    }
}
