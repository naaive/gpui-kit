//! Placeholders in quicklinks and snippets, filled in when they are used:
//! `{argument}` (or `{query}`) asks for text, `{argument name="city"
//! default="Paris"}` names one of several, and `{clipboard}`, `{date}`,
//! `{time}`, `{datetime}` and `{uuid}` insert what they say. `{cursor}`
//! marks where a snippet's text ends up and inserts nothing.

use chrono::Local;

/// A value asked for when the text is used.
#[derive(Clone, Debug, PartialEq)]
pub struct Argument {
    /// Empty for the unnamed `{argument}` / `{query}`.
    pub name: String,
    pub default: String,
}

impl Argument {
    pub fn title(&self) -> String {
        match self.name.is_empty() {
            true => "Query".into(),
            false => self.name.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Placeholder {
    Argument(Argument),
    Clipboard,
    Date,
    Time,
    DateTime,
    Uuid,
    Cursor,
}

/// The arguments `template` asks for, once each, in order of appearance.
pub fn arguments(template: &str) -> Vec<Argument> {
    let mut arguments: Vec<Argument> = Vec::new();
    let found = template
        .split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}'))
        .filter_map(|(inside, _)| parse(inside));
    for placeholder in found {
        if let Placeholder::Argument(argument) = placeholder
            && !arguments.iter().any(|known| known.name == argument.name)
        {
            arguments.push(argument);
        }
    }
    arguments
}

/// `template` with every placeholder filled in, each inserted value passed
/// through `insert` (which percent-encodes it in a URL). `values` holds an
/// argument's value by name; a missing or empty one takes its default.
/// Text in braces that is not a placeholder stays as written.
pub fn expand(
    template: &str,
    values: &[(String, String)],
    clipboard: Option<&str>,
    insert: impl Fn(&str) -> String,
) -> String {
    let now = Local::now();
    let mut expanded = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let Some(length) = rest[start..].find('}') else {
            break;
        };
        expanded.push_str(&rest[..start]);
        let token = &rest[start..=start + length];
        let value = match parse(&token[1..token.len() - 1]) {
            Some(Placeholder::Argument(argument)) => Some(
                values
                    .iter()
                    .find(|(name, _)| *name == argument.name)
                    .map(|(_, value)| value.clone())
                    .filter(|value| !value.is_empty())
                    .unwrap_or(argument.default),
            ),
            Some(Placeholder::Clipboard) => Some(clipboard.unwrap_or_default().to_owned()),
            Some(Placeholder::Date) => Some(now.format("%Y-%m-%d").to_string()),
            Some(Placeholder::Time) => Some(now.format("%H:%M").to_string()),
            Some(Placeholder::DateTime) => Some(now.format("%Y-%m-%d %H:%M").to_string()),
            Some(Placeholder::Uuid) => Some(uuid()),
            Some(Placeholder::Cursor) => Some(String::new()),
            None => None,
        };
        match value {
            Some(value) => {
                expanded.push_str(&insert(&value));
                rest = &rest[start + length + 1..];
            }
            // Not a placeholder: keep the brace and look again after it, so
            // `{x {argument}` still fills in the argument.
            None => {
                expanded.push('{');
                rest = &rest[start + 1..];
            }
        }
    }
    expanded.push_str(rest);
    expanded
}

/// Reads `argument name="city" default="Paris"`; `None` for text that is
/// not a placeholder.
fn parse(inside: &str) -> Option<Placeholder> {
    let inside = inside.trim();
    let (keyword, attributes) = inside.split_once(' ').unwrap_or((inside, ""));
    let attribute = |key: &str| {
        let marker = format!("{key}=\"");
        let start = attributes.find(&marker)? + marker.len();
        let end = attributes[start..].find('"')?;
        Some(attributes[start..start + end].to_owned())
    };
    Some(match keyword {
        "argument" | "query" => Placeholder::Argument(Argument {
            name: attribute("name").unwrap_or_default(),
            default: attribute("default").unwrap_or_default(),
        }),
        "clipboard" => Placeholder::Clipboard,
        "date" => Placeholder::Date,
        "time" => Placeholder::Time,
        "datetime" => Placeholder::DateTime,
        "uuid" => Placeholder::Uuid,
        "cursor" => Placeholder::Cursor,
        _ => return None,
    })
}

/// A random version 4 UUID, from the standard library's randomly keyed hasher.
fn uuid() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let random = || {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default(),
        );
        hasher.finish()
    };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&random().to_le_bytes());
    bytes[8..].copy_from_slice(&random().to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_snippet_placeholders() {
        let text = expand(
            "Hi {argument name=\"who\" default=\"there\"}{cursor}, {clipboard}! {nope}",
            &[],
            Some("copied"),
            str::to_owned,
        );
        assert_eq!(text, "Hi there, copied! {nope}");
        let template = "{x {argument}";
        assert_eq!(arguments(template).len(), 1);
        assert_eq!(
            expand(
                template,
                &[(String::new(), "1".into())],
                None,
                str::to_owned
            ),
            "{x 1"
        );
        let id = expand("{uuid}", &[], None, str::to_owned);
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert_ne!(id, expand("{uuid}", &[], None, str::to_owned));
    }
}
