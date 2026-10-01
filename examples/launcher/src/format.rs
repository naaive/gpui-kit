//! Formatting shared by built-in pages: sizes, times and verbatim text.

use chrono::{DateTime, Local, TimeZone as _};

/// `text` as a fenced block, fenced with more backticks than it contains, so
/// nothing in it is read as Markdown.
pub fn code_block(text: &str) -> String {
    code_block_in(text, "text")
}

/// `text` as a fenced block highlighted as `language`, such as `rust`.
pub fn code_block_in(text: &str, language: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}{language}\n{text}\n{fence}")
}

/// Unix seconds as a local time.
pub fn local(seconds: u64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(seconds as i64, 0).single()
}

/// How long ago `seconds` was, briefly: “5m ago”, or the date.
pub fn relative_time(copied_at: u64, now: u64) -> String {
    let seconds = now.saturating_sub(copied_at);
    match seconds {
        0..60 => "Just now".into(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        _ => match local(copied_at) {
            Some(time) => time.format("%b %-d").to_string(),
            None => String::new(),
        },
    }
}

/// A full local date and time.
pub fn format_time(seconds: u64) -> String {
    local(seconds)
        .map(|time| time.format("%b %-d, %Y at %H:%M").to_string())
        .unwrap_or_default()
}

/// A byte count in B, KB, MB or GB.
pub fn format_bytes(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.),
        1_048_576..1_073_741_824 => format!("{:.1} MB", bytes as f64 / 1_048_576.),
        _ => format!("{:.1} GB", bytes as f64 / 1_073_741_824.),
    }
}
