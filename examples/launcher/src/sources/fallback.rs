//! What to do with a query when the list is not the answer: search the web,
//! or hand the text to an extension command that declared `"fallback": true`.

use gpui_kit::SharedString;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::{
    extensions::{Catalog, CommandId, LaunchRequest},
    model::{Accessory, Action, Effect, Item, ItemId, Section},
};

/// Everything except RFC 3986's unreserved characters is escaped, so a query
/// such as `a&b=c #1` cannot add parameters or a fragment to the URL.
const QUERY_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// An extension command offered with the query.
#[derive(Clone, Debug)]
pub struct FallbackCommand {
    command: CommandId,
    title: SharedString,
    extension: SharedString,
    icon: Option<SharedString>,
    /// The command's first declared argument, which receives the query.
    argument: Option<SharedString>,
}

impl FallbackCommand {
    /// Every command in `catalog` that declared itself a fallback.
    pub fn from_catalog(catalog: &Catalog) -> Vec<Self> {
        catalog
            .commands()
            .filter(|(_, command)| command.is_fallback())
            .map(|(extension, command)| Self {
                command: command.id().clone(),
                title: command.title().clone(),
                extension: extension.name().clone(),
                icon: command.icon().cloned(),
                argument: command
                    .arguments()
                    .first()
                    .map(|argument| argument.name.clone().into()),
            })
            .collect()
    }

    fn item(&self, query: &str) -> Item {
        let request = LaunchRequest::new(self.command.clone());
        let request = match &self.argument {
            Some(argument) => request.with_argument(argument.clone(), query.to_owned()),
            None => request,
        };
        Item::new(
            ItemId::new(format!("fallback/{}", self.command)),
            self.title.clone(),
        )
        .with_subtitle(self.extension.clone())
        .with_icon(self.icon.clone().unwrap_or_else(|| "puzzle".into()))
        .with_accessory(Accessory::text("Command"))
        .with_action(Action::new("Open Command", Effect::Launch(request)))
    }
}

/// The section after the results: `Use “query” with…`, or `None` for an empty
/// query.
pub fn section(query: &str, commands: &[FallbackCommand]) -> Option<Section> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let url = google_url(query);
    let google = Item::new(ItemId::new("fallback/google"), "Search Google")
        .with_icon("globe")
        .with_accessory(Accessory::text("Web"))
        .with_action(Action::new("Open in Browser", Effect::OpenUrl(url.clone())))
        .with_action(Action::new("Copy URL", Effect::Copy(url)));
    Some(
        Section::new()
            .with_title(format!("Use “{query}” with…"))
            .with_item(google)
            .with_items(commands.iter().map(|command| command.item(query))),
    )
}

pub fn google_url(query: &str) -> SharedString {
    format!(
        "https://www.google.com/search?q={}",
        utf8_percent_encode(query, QUERY_COMPONENT)
    )
    .into()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_query_is_encoded_into_the_search_url() {
        assert_eq!(
            google_url("rust & gpui #1 微信").as_ref(),
            "https://www.google.com/search?q=rust%20%26%20gpui%20%231%20%E5%BE%AE%E4%BF%A1"
        );
    }

    #[test]
    fn test_fallback_commands_receive_the_query_as_their_first_argument() {
        let root = tempfile::tempdir().unwrap();
        let extension = root.path().join("search");
        std::fs::create_dir_all(&extension).unwrap();
        std::fs::write(
            extension.join("gpui-shell.json"),
            r#"{ "id": "test.search", "name": "Search", "entry": "main.js" }"#,
        )
        .unwrap();
        std::fs::write(
            extension.join("launcher.json"),
            r#"{ "commands": [
                { "name": "docs", "title": "Search Docs", "module": "main.js", "fallback": true,
                  "arguments": [{ "name": "term", "placeholder": "Term" }] },
                { "name": "plain", "title": "Plain", "module": "main.js" }
            ] }"#,
        )
        .unwrap();
        std::fs::write(extension.join("main.js"), "").unwrap();
        let catalog = Catalog::discover(&[PathBuf::from(root.path())]);
        let commands = FallbackCommand::from_catalog(&catalog);

        assert!(section("  ", &commands).is_none());
        let section = section(" gpui ", &commands).unwrap();
        assert_eq!(
            section.title().map(|t| t.as_ref()),
            Some("Use “gpui” with…")
        );
        let titles: Vec<&str> = section
            .items()
            .iter()
            .map(|item| item.title().as_ref())
            .collect();
        assert_eq!(titles, ["Search Google", "Search Docs"]);
        let Effect::Launch(request) = section.items()[1].primary_action().unwrap().effect() else {
            panic!("a fallback command is launched");
        };
        assert_eq!(request.command().to_string(), "test.search/docs");
        assert_eq!(
            request.arguments().get("term").map(|v| v.as_ref()),
            Some("gpui")
        );
    }
}
