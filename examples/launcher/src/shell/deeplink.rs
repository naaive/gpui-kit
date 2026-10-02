//! `launcher://` deep links.
//!
//! `launcher://extensions/<extension-id>/<command-name>?arguments=<JSON>`
//! opens a command, the way Raycast's deep links do, so a script, a browser
//! or another application can start one. `arguments` is a URL-encoded JSON
//! object whose values fill the command's arguments; `context` is any
//! URL-encoded JSON, which the command reads as `launch().context`.

use anyhow::{Context as _, Result, anyhow, bail};
use percent_encoding::percent_decode_str;
use url::Url;

use crate::extensions::{CommandId, LaunchRequest};

pub const SCHEME: &str = "launcher";

/// What `launcher://settings` asks for: the settings window, at the
/// extension or command in the path when there is one
/// (`launcher://settings/com.example.notes`).
pub fn settings_target(link: &str) -> Option<Option<String>> {
    let url = Url::parse(link).ok()?;
    if url.scheme() != SCHEME || url.host_str() != Some("settings") {
        return None;
    }
    let target = url.path().trim_matches('/').to_owned();
    let target = percent_decode_str(&target).decode_utf8().ok()?.into_owned();
    Some((!target.is_empty()).then_some(target))
}

/// Parses a deep link into the request it makes.
pub fn parse(link: &str) -> Result<LaunchRequest> {
    let url = Url::parse(link).with_context(|| format!("`{link}` is not a URL"))?;
    if url.scheme() != SCHEME {
        bail!("`{link}` is not a `{SCHEME}://` link");
    }
    if url.host_str() != Some("extensions") {
        bail!(
            "`{link}` does not name a command: expected `{SCHEME}://extensions/<extension-id>/<command>`"
        );
    }
    let segments = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            percent_decode_str(segment)
                .decode_utf8()
                .map(|segment| segment.into_owned())
                .with_context(|| format!("`{link}` is not UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;
    let [extension, command] = segments.as_slice() else {
        bail!("`{link}` must name exactly an extension and a command");
    };
    let mut request = LaunchRequest::new(CommandId::new(extension.clone(), command.clone()));
    if let Some((_, context)) = url.query_pairs().find(|(key, _)| key == "context") {
        request = request.with_context(
            serde_json::from_str(&context)
                .with_context(|| format!("the `context` of `{link}` is not JSON"))?,
        );
    }

    let Some((_, arguments)) = url.query_pairs().find(|(key, _)| key == "arguments") else {
        return Ok(request);
    };
    let arguments: serde_json::Value = serde_json::from_str(&arguments)
        .with_context(|| format!("the `arguments` of `{link}` are not JSON"))?;
    let serde_json::Value::Object(arguments) = arguments else {
        bail!("the `arguments` of `{link}` must be a JSON object");
    };
    arguments
        .into_iter()
        .try_fold(request, |request, (name, value)| {
            let value = match value {
                serde_json::Value::String(text) => text,
                serde_json::Value::Number(number) => number.to_string(),
                serde_json::Value::Bool(flag) => flag.to_string(),
                other => {
                    return Err(anyhow!(
                        "argument `{name}` must be a string, number or boolean, not `{other}`"
                    ));
                }
            };
            Ok(request.with_argument(name, value))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_links() {
        assert_eq!(settings_target("launcher://settings"), Some(None));
        assert_eq!(
            settings_target("launcher://settings/com.example.notes"),
            Some(Some("com.example.notes".into()))
        );
        assert_eq!(
            settings_target("launcher://settings/system%2Fclipboard-history"),
            Some(Some("system/clipboard-history".into()))
        );
        assert_eq!(settings_target("launcher://extensions/a/b"), None);
    }

    #[test]
    fn test_parse_a_command_link() {
        let request = parse("launcher://extensions/com.gpui-kit.links/checklist").unwrap();
        assert_eq!(request.command().extension().as_ref(), "com.gpui-kit.links");
        assert_eq!(request.command().command().as_ref(), "checklist");
        assert!(request.arguments().is_empty());

        // A built-in item's full id travels as one escaped segment.
        let request = parse("launcher://extensions/launcher/settings%2Fdisplay").unwrap();
        assert_eq!(request.command().command().as_ref(), "settings/display");
    }

    #[test]
    fn test_parse_arguments() {
        let request = parse(
            "launcher://extensions/com.example/search?arguments=%7B%22query%22%3A%22a%20b%2Bc%22%2C%22count%22%3A3%2C%22exact%22%3Atrue%7D",
        )
        .unwrap();
        let arguments: Vec<_> = request
            .arguments()
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        assert_eq!(arguments, ["count=3", "exact=true", "query=a b+c"]);

        let request =
            parse("launcher://extensions/com.example/open?context=%7B%22id%22%3A7%7D").unwrap();
        assert_eq!(request.context(), Some(&serde_json::json!({ "id": 7 })));
        assert!(parse("launcher://extensions/com.example/open?context=nope").is_err());
    }

    #[test]
    fn test_parse_decodes_the_path() {
        let request = parse("launcher://extensions/com.example/say%20hi/").unwrap();
        assert_eq!(request.command().command().as_ref(), "say hi");
    }

    #[test]
    fn test_parse_rejects_malformed_links() {
        for link in [
            "not a url",
            "https://extensions/com.example/hello",
            "launcher://commands/com.example/hello",
            "launcher://extensions/com.example",
            "launcher://extensions/com.example/hello/extra",
            "launcher://extensions/com.example/hello?arguments=nope",
            "launcher://extensions/com.example/hello?arguments=%5B1%5D",
            "launcher://extensions/com.example/hello?arguments=%7B%22a%22%3A%7B%7D%7D",
        ] {
            assert!(parse(link).is_err(), "{link}");
        }
    }
}
