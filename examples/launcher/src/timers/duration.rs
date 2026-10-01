//! Reading a timer's length from what the user types, and writing lengths
//! and clocks back.
//!
//! Accepted lengths: `25m`, `1h 30m`, `1h30`, `90s`, `1.5h`, `10:00`
//! (minutes and seconds), `1:30:00`, `25` (minutes), `5 minutes`, `10分钟`,
//! `半小时`, `一个半小时`, `一刻钟`. A name may come after the length or before
//! it: `25m tea`, `tea 25m`, `10分钟泡茶`.

/// The longest timer: just under 100 hours.
pub const LONGEST: u64 = 100 * 3600 - 1;

const HOUR: u64 = 3600;
const MINUTE: u64 = 60;
const SECOND: u64 = 1;

/// Units and their length in seconds. English units must end a word, so
/// `5 mangoes` is five minutes named "mangoes", not five minutes "angoes".
const ENGLISH_UNITS: [(&str, u64); 15] = [
    ("hours", HOUR),
    ("hour", HOUR),
    ("hrs", HOUR),
    ("hr", HOUR),
    ("h", HOUR),
    ("minutes", MINUTE),
    ("minute", MINUTE),
    ("mins", MINUTE),
    ("min", MINUTE),
    ("m", MINUTE),
    ("seconds", SECOND),
    ("second", SECOND),
    ("secs", SECOND),
    ("sec", SECOND),
    ("s", SECOND),
];

const CHINESE_UNITS: [(&str, u64); 8] = [
    ("小时", HOUR),
    ("钟头", HOUR),
    ("分钟", MINUTE),
    ("分", MINUTE),
    ("秒钟", SECOND),
    ("秒", SECOND),
    ("刻钟", 15 * MINUTE),
    ("刻", 15 * MINUTE),
];

/// A timer's length in seconds and its name, empty when none was typed.
pub fn parse_query(query: &str) -> Option<(u64, String)> {
    let query = query.trim();
    if let Some((seconds, rest)) = parse_prefix(query) {
        return Some((seconds, rest.trim().to_owned()));
    }
    // The name first: `tea 25m`.
    query
        .char_indices()
        .filter(|(_, character)| character.is_whitespace())
        .find_map(|(ix, _)| {
            let (name, tail) = query.split_at(ix);
            let (seconds, rest) = parse_prefix(tail.trim_start())?;
            rest.trim()
                .is_empty()
                .then(|| (seconds, name.trim().to_owned()))
        })
}

/// The length at the start of `text` in seconds, and what follows it.
fn parse_prefix(text: &str) -> Option<(u64, &str)> {
    let (seconds, rest) = match parse_clock(text) {
        Some(clock) => clock,
        None => parse_units(text)?,
    };
    (1..=LONGEST).contains(&seconds).then_some((seconds, rest))
}

/// `10:00` (minutes and seconds) or `1:30:00`.
fn parse_clock(text: &str) -> Option<(u64, &str)> {
    let end = text
        .find(|character: char| !(character.is_ascii_digit() || character == ':'))
        .unwrap_or(text.len());
    let (clock, rest) = text.split_at(end);
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let parts: Vec<&str> = clock.split(':').collect();
    if !(2..=3).contains(&parts.len())
        || parts[0].is_empty()
        || parts[1..].iter().any(|part| part.len() != 2)
    {
        return None;
    }
    let numbers: Vec<u64> = parts
        .iter()
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    if numbers[1..].iter().any(|&number| number >= 60) {
        return None;
    }
    let seconds = numbers.iter().fold(0u64, |total, &number| {
        total.saturating_mul(60).saturating_add(number)
    });
    Some((seconds, rest))
}

/// Numbers with units: `1h 30m`, `1小时30分`, `25`, `1h30`.
fn parse_units(text: &str) -> Option<(u64, &str)> {
    let mut total = 0.0_f64;
    let mut rest = text;
    // The unit of the last number read, once one has been.
    let mut last: Option<u64> = None;
    while let Some((mut value, after_number)) = number(rest.trim_start()) {
        let mut after = after_number.trim_start();
        // 两个小时, 一个半小时, 半个小时
        if let Some(measure) = after.strip_prefix('个') {
            after = measure.trim_start();
            if let Some(half) = after.strip_prefix('半') {
                value += 0.5;
                after = half;
            }
        }
        if let Some((unit, after_unit)) = unit(after) {
            total += value * unit as f64;
            last = Some(unit);
            rest = after_unit;
            continue;
        }
        // A number without a unit is minutes on its own, and the next smaller
        // unit after another: `1h30`, `5m30`, `1小时30`. Only digits stand
        // alone: `半` or `两` is not a length.
        let is_digits = rest
            .trim_start()
            .starts_with(|character: char| character.is_ascii_digit());
        let ends_word = after_number.is_empty()
            || after_number.starts_with(char::is_whitespace)
            || !after_number.starts_with(|character: char| character.is_ascii());
        if !(is_digits && ends_word) {
            break;
        }
        let unit = match last {
            None => MINUTE,
            Some(HOUR) => MINUTE,
            Some(MINUTE) => SECOND,
            Some(_) => break,
        };
        total += value * unit as f64;
        last = Some(unit);
        rest = after_number;
        break;
    }
    last?;
    if !total.is_finite() || total > LONGEST as f64 {
        return None;
    }
    Some((total.round() as u64, rest))
}

/// A number at the start of `text`: `25`, `1.5`, `十五`, `两`, `半`.
fn number(text: &str) -> Option<(f64, &str)> {
    let digits = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    if digits > 0 {
        let (whole, rest) = text.split_at(digits);
        // A decimal part: `1.5h`.
        if let Some(fraction) = rest.strip_prefix('.') {
            let length = fraction
                .find(|character: char| !character.is_ascii_digit())
                .unwrap_or(fraction.len());
            if length > 0 {
                let value = format!("{whole}.{}", &fraction[..length]).parse().ok()?;
                return Some((value, &fraction[length..]));
            }
        }
        return Some((whole.parse().ok()?, rest));
    }
    if let Some(rest) = text.strip_prefix('半') {
        return Some((0.5, rest));
    }
    chinese_number(text)
}

/// A Chinese number below a hundred: `五`, `两`, `十`, `十五`, `二十`, `四十五`.
fn chinese_number(text: &str) -> Option<(f64, &str)> {
    let digit = |character: char| {
        "零一二三四五六七八九"
            .chars()
            .position(|known| known == character)
            .map(|position| position as u64)
            .or((character == '两').then_some(2))
    };
    let mut characters = text.char_indices().peekable();
    let mut end = 0;
    let mut tens = None;
    let mut ones = None;
    while let Some(&(ix, character)) = characters.peek() {
        if character == '十' && tens.is_none() {
            tens = Some(ones.take().unwrap_or(1));
        } else if let Some(digit) = digit(character) {
            if ones.is_some() {
                break;
            }
            ones = Some(digit);
        } else {
            break;
        }
        end = ix + character.len_utf8();
        characters.next();
    }
    if end == 0 {
        return None;
    }
    let value = tens.unwrap_or(0) * 10 + ones.unwrap_or(0);
    Some((value as f64, &text[end..]))
}

/// The unit at the start of `text`, in seconds, and what follows it.
fn unit(text: &str) -> Option<(u64, &str)> {
    let english = ENGLISH_UNITS.iter().find_map(|&(name, seconds)| {
        let prefix = text.get(..name.len())?;
        let rest = &text[name.len()..];
        (prefix.eq_ignore_ascii_case(name)
            && !rest.starts_with(|character: char| character.is_ascii_alphabetic()))
        .then_some((seconds, rest))
    });
    english.or_else(|| {
        CHINESE_UNITS
            .iter()
            .find_map(|&(name, seconds)| text.strip_prefix(name).map(|rest| (seconds, rest)))
    })
}

fn plural(count: u64, unit: &str) -> String {
    match count {
        1 => format!("1 {unit}"),
        count => format!("{count} {unit}s"),
    }
}

/// `25 minutes`, `1 hour 30 minutes`, `45 seconds`.
pub fn format_length(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    let parts: Vec<String> = [(hours, "hour"), (minutes, "minute"), (seconds, "second")]
        .into_iter()
        .filter(|&(count, _)| count > 0)
        .map(|(count, unit)| plural(count, unit))
        .collect();
    match parts.is_empty() {
        true => "0 seconds".into(),
        false => parts.join(" "),
    }
}

/// The name of a timer the user did not name: `25-minute timer`,
/// `1-hour timer`, `1 h 30 min timer`.
pub fn default_name(seconds: u64) -> String {
    let (hours, minutes, rest) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    let single = |count: u64, unit: &str| format!("{count}-{unit} timer");
    match (hours, minutes, rest) {
        (0, 0, seconds) => single(seconds, "second"),
        (0, minutes, 0) => single(minutes, "minute"),
        (hours, 0, 0) => single(hours, "hour"),
        (hours, minutes, seconds) => {
            let parts: Vec<String> = [(hours, "h"), (minutes, "min"), (seconds, "s")]
                .into_iter()
                .filter(|&(count, _)| count > 0)
                .map(|(count, unit)| format!("{count} {unit}"))
                .collect();
            format!("{} timer", parts.join(" "))
        }
    }
}

/// A countdown or a stopwatch reading: `0:05`, `12:03`, `1:02:03`.
pub fn format_clock(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}"),
        hours => format!("{hours}:{minutes:02}:{seconds:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn length(query: &str) -> Option<u64> {
        parse_query(query).map(|(seconds, _)| seconds)
    }

    #[test]
    fn test_parses_english_lengths() {
        assert_eq!(length("25m"), Some(25 * 60));
        assert_eq!(length("1h 30m"), Some(90 * 60));
        assert_eq!(length("1h30m"), Some(90 * 60));
        assert_eq!(length("1h30"), Some(90 * 60));
        assert_eq!(length("5m30"), Some(330));
        assert_eq!(length("90s"), Some(90));
        assert_eq!(length("1.5h"), Some(90 * 60));
        assert_eq!(length("10:00"), Some(600));
        assert_eq!(length("1:30:00"), Some(90 * 60));
        assert_eq!(length("25"), Some(25 * 60));
        assert_eq!(length("5 minutes"), Some(300));
        assert_eq!(length("1 Hour 5 Minutes"), Some(65 * 60));
        assert_eq!(length("2 hrs"), Some(7200));
        assert_eq!(length("  45 sec "), Some(45));
    }

    #[test]
    fn test_parses_chinese_lengths() {
        assert_eq!(length("10分钟"), Some(600));
        assert_eq!(length("半小时"), Some(1800));
        assert_eq!(length("半个小时"), Some(1800));
        assert_eq!(length("一个半小时"), Some(90 * 60));
        assert_eq!(length("两分钟"), Some(120));
        assert_eq!(length("十五分钟"), Some(15 * 60));
        assert_eq!(length("二十五分钟"), Some(25 * 60));
        assert_eq!(length("一刻钟"), Some(15 * 60));
        assert_eq!(length("1小时30分"), Some(90 * 60));
        assert_eq!(length("1小时30"), Some(90 * 60));
        assert_eq!(length("30秒"), Some(30));
    }

    #[test]
    fn test_parses_names() {
        assert_eq!(parse_query("25m tea"), Some((1500, "tea".into())));
        assert_eq!(parse_query("25 tea"), Some((1500, "tea".into())));
        assert_eq!(parse_query("10:00 Pasta"), Some((600, "Pasta".into())));
        assert_eq!(parse_query("tea 25m"), Some((1500, "tea".into())));
        assert_eq!(
            parse_query("green tea 3 minutes"),
            Some((180, "green tea".into()))
        );
        assert_eq!(parse_query("10分钟泡茶"), Some((600, "泡茶".into())));
        assert_eq!(parse_query("5 mangoes"), Some((300, "mangoes".into())));
        assert_eq!(parse_query("1h 30m"), Some((5400, String::new())));
    }

    #[test]
    fn test_rejects_what_is_not_a_length() {
        for query in [
            "", "   ", "tea", "0", "0m", "0:00", "25x", "-5m", "10:75", "1:5", ":30", "m", "100h",
            "分钟", "半",
        ] {
            assert_eq!(parse_query(query), None, "{query:?}");
        }
    }

    #[test]
    fn test_formats_lengths_and_clocks() {
        assert_eq!(format_length(1500), "25 minutes");
        assert_eq!(format_length(60), "1 minute");
        assert_eq!(format_length(5400), "1 hour 30 minutes");
        assert_eq!(format_length(90), "1 minute 30 seconds");
        assert_eq!(default_name(1500), "25-minute timer");
        assert_eq!(default_name(3600), "1-hour timer");
        assert_eq!(default_name(45), "45-second timer");
        assert_eq!(default_name(5400), "1 h 30 min timer");
        assert_eq!(format_clock(5), "0:05");
        assert_eq!(format_clock(723), "12:03");
        assert_eq!(format_clock(3723), "1:02:03");
    }
}
