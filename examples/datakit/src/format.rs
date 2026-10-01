//! How DataKit writes numbers, durations and times in the interface.

use std::time::Duration;

use chrono::{Local, TimeZone as _};
use gpui_kit::SharedString;

/// `1234567` as `1,234,567`.
pub fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A statement's duration: milliseconds under a second, seconds under a
/// minute, minutes beyond.
pub fn duration(elapsed: Duration) -> SharedString {
    let ms = elapsed.as_millis();
    if ms < 1000 {
        format!("{ms} ms").into()
    } else if ms < 60_000 {
        format!("{:.2} s", elapsed.as_secs_f64()).into()
    } else {
        let seconds = elapsed.as_secs();
        format!("{} m {} s", seconds / 60, seconds % 60).into()
    }
}

/// Milliseconds since the Unix epoch as a local time of day.
pub fn time_of_day(ms: i64) -> SharedString {
    match Local.timestamp_millis_opt(ms).single() {
        Some(time) => time.format("%H:%M:%S").to_string().into(),
        None => SharedString::default(),
    }
}

/// Milliseconds since the Unix epoch as a local date and time.
pub fn date_time(ms: i64) -> SharedString {
    match Local.timestamp_millis_opt(ms).single() {
        Some(time) => time.format("%Y-%m-%d %H:%M:%S").to_string().into(),
        None => SharedString::default(),
    }
}

/// Now, in milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(1234567), "1,234,567");
    }

    #[test]
    fn durations_pick_a_readable_unit() {
        assert_eq!(duration(Duration::from_millis(12)), "12 ms");
        assert_eq!(duration(Duration::from_millis(1500)), "1.50 s");
        assert_eq!(duration(Duration::from_secs(125)), "2 m 5 s");
    }
}
