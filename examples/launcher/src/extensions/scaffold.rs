//! `launcher new` and `launcher lint`: starting an extension from a template,
//! and checking one before it is shared, without running its code.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use gpui_shell::plugin::PluginManifest;

use super::manifest::{CommandMode, LauncherManifest};

/// What a new extension's first command is.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Template {
    #[default]
    List,
    Detail,
    Form,
    NoView,
    MenuBar,
}

impl Template {
    pub const NAMES: &[&str] = &["list", "detail", "form", "no-view", "menu-bar"];

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "list" => Self::List,
            "detail" => Self::Detail,
            "form" => Self::Form,
            "no-view" => Self::NoView,
            "menu-bar" => Self::MenuBar,
            _ => return None,
        })
    }

    fn mode(self) -> Option<&'static str> {
        match self {
            Self::List | Self::Detail | Self::Form => None,
            Self::NoView => Some("no-view"),
            Self::MenuBar => Some("menu-bar"),
        }
    }

    fn source(self) -> &'static str {
        match self {
            Self::List => LIST,
            Self::Detail => DETAIL,
            Self::Form => FORM,
            Self::NoView => NO_VIEW,
            Self::MenuBar => MENU_BAR,
        }
    }
}

/// Writes a new extension into `directory`, which must be empty or absent,
/// with its declarations; answers the files written.
pub fn create(directory: &Path, template: Template) -> Result<Vec<PathBuf>> {
    if directory
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some())
    {
        bail!("{} is not empty", directory.display());
    }
    let name = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        bail!("name the extension's directory with letters or digits, such as `hello`");
    }
    let title = slug
        .split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let module = format!("commands/{slug}.js");
    let mode = template
        .mode()
        .map(|mode| format!(",\n      \"mode\": \"{mode}\""))
        .unwrap_or_default();
    let interval = match template {
        Template::MenuBar => ",\n      \"interval\": \"5m\"",
        _ => "",
    };
    let files = [
        (
            "gpui-shell.json".to_owned(),
            format!(
                "{{\n  \"id\": \"com.example.{slug}\",\n  \"name\": \"{title}\",\n  \"version\": \"0.1.0\",\n  \"entry\": \"{module}\"\n}}\n"
            ),
        ),
        (
            "launcher.json".to_owned(),
            format!(
                "{{\n  \"$schema\": \"./launcher.schema.json\",\n  \"icon\": \"sparkles\",\n  \"commands\": [\n    {{\n      \"name\": \"{slug}\",\n      \"title\": \"{title}\",\n      \"module\": \"{module}\"{mode}{interval}\n    }}\n  ]\n}}\n"
            ),
        ),
        (module.clone(), template.source().replace("TITLE", &title)),
    ];
    let mut written = Vec::new();
    for (path, contents) in files {
        let path = directory.join(path);
        std::fs::create_dir_all(path.parent().expect("inside the directory"))?;
        std::fs::write(&path, contents)
            .with_context(|| format!("cannot write {}", path.display()))?;
        written.push(path);
    }
    written.extend(super::write_declarations(directory)?);
    Ok(written)
}

/// One thing `lint` found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Problem {
    pub error: bool,
    pub message: String,
}

impl Problem {
    fn error(message: impl Into<String>) -> Self {
        Self {
            error: true,
            message: message.into(),
        }
    }

    fn warning(message: impl Into<String>) -> Self {
        Self {
            error: false,
            message: message.into(),
        }
    }
}

/// Checks an extension as the launcher reads it, without running it: both
/// manifests, every module and image they name, and that each module
/// default-exports a View.
pub fn lint(directory: &Path) -> Vec<Problem> {
    let mut problems = Vec::new();
    if let Err(error) = PluginManifest::read(directory) {
        problems.push(Problem::error(format!("gpui-shell.json: {error:#}")));
    }
    let manifest = match LauncherManifest::read(directory) {
        Ok(manifest) => manifest,
        Err(error) => {
            problems.push(Problem::error(format!("{error:#}")));
            return problems;
        }
    };
    let image = |at: &str, icon: &str, problems: &mut Vec<Problem>| {
        let looks_like_file = icon.contains('/') || icon.contains('.');
        if looks_like_file && !directory.join(icon).is_file() {
            problems.push(Problem::error(format!("{at}: no file `{icon}`")));
        }
    };
    if let Some(icon) = &manifest.icon {
        image("launcher.json icon", icon, &mut problems);
    }
    for command in &manifest.commands {
        let at = format!("command `{}`", command.name);
        if let Some(icon) = &command.icon {
            image(&format!("{at} icon"), icon, &mut problems);
        }
        let path = directory.join(&command.module);
        let Ok(source) = std::fs::read_to_string(&path) else {
            problems.push(Problem::error(format!(
                "{at}: no module `{}`",
                command.module
            )));
            continue;
        };
        if !source.contains("export default") {
            problems.push(Problem::error(format!(
                "{at}: `{}` must `export default` a class extending `View`",
                command.module
            )));
        }
        let returns = match command.mode {
            CommandMode::MenuBar => "MenuBarExtra",
            CommandMode::View => "List",
            CommandMode::NoView => "",
        };
        let returns_page = ["List", "Detail", "Form"]
            .iter()
            .any(|node| source.contains(&format!("new {node}(")));
        match command.mode {
            CommandMode::MenuBar if !source.contains("new MenuBarExtra(") => {
                problems.push(Problem::warning(format!(
                    "{at}: a `menu-bar` command's `render` returns a `{returns}`"
                )));
            }
            CommandMode::View if !returns_page => problems.push(Problem::warning(format!(
                "{at}: `render` should return a `List`, `Detail` or `Form`"
            ))),
            _ => {}
        }
    }
    if !directory.join("launcher.d.ts").is_file() {
        problems.push(Problem::warning(
            "no TypeScript declarations; `launcher types <dir>` writes them",
        ));
    }
    problems
}

const LIST: &str = r#"import { View } from "gpui-kit";
import { Action, ActionPanel, List, ListItem } from "launcher";

const ITEMS = ["Apples", "Bananas", "Cherries"];

export default class Command extends View {
  render() {
    return new List().placeholder("Search fruit…").children(
      ITEMS.map((name) =>
        new ListItem(name.toLowerCase(), name)
          .icon("apple")
          .actions(
            new ActionPanel().children([
              new Action("Copy Name").copy(name),
              new Action("Search the Web").open_url(
                `https://duckduckgo.com/?q=${encodeURIComponent(name)}`,
              ),
            ]),
          ),
      ),
    );
  }
}
"#;

const DETAIL: &str = r##"import { View } from "gpui-kit";
import { Action, ActionPanel, Detail, MetadataLabel } from "launcher";

export default class Command extends View {
  render() {
    return new Detail("# TITLE\n\nWrite Markdown here.")
      .child(new MetadataLabel("Status", "Ready"))
      .actions(new ActionPanel().child(new Action("Copy Title").copy("TITLE")));
  }
}
"##;

const FORM: &str = r#"import { View } from "gpui-kit";
import { Action, ActionPanel, Form, TextField } from "launcher";
import { show_toast } from "launcher/api";
import { FormState } from "launcher/utils";

export default class Command extends View {
  init(_props, cx) {
    this.form = new FormState({ name: "" }, { name: FormState.required("Enter a name") });
  }

  render(cx) {
    return new Form()
      .child(this.form.bind("name", new TextField("name", "Name").placeholder("Ada")))
      .actions(
        new ActionPanel().child(
          new Action("Say Hello").submit((values, cx) => {
            if (this.form.validate(values)) {
              show_toast({ title: `Hello, ${values.name}`, style: "success" });
            }
            cx.notify();
          }),
        ),
      );
  }
}
"#;

const NO_VIEW: &str = r#"import { View } from "gpui-kit";
import { Detail } from "launcher";
import { show_hud } from "launcher/api";

export default class Command extends View {
  init() {
    show_hud("TITLE ran");
  }

  render() {
    return new Detail("");
  }
}
"#;

const MENU_BAR: &str = r#"import { View } from "gpui-kit";
import { Action, MenuBarExtra, MenuBarItem, MenuBarSeparator } from "launcher";
import { launch } from "launcher/api";

export default class Command extends View {
  init() {
    this.ran = new Date();
  }

  render() {
    return new MenuBarExtra()
      .icon("sparkles")
      .tooltip("TITLE")
      .children([
        new MenuBarItem(`Updated ${this.ran.toLocaleTimeString()}`),
        new MenuBarSeparator(),
        new MenuBarItem("Open GPUI Kit").action(
          new Action("Open").open_url("https://gpui-kit.com"),
        ),
        new MenuBarItem(`Launched by ${launch().launch_type}`),
      ]);
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_template_passes_lint() {
        for name in Template::NAMES {
            let folder = tempfile::tempdir().unwrap();
            let directory = folder.path().join(format!("my-{name}"));
            create(&directory, Template::parse(name).unwrap()).unwrap();
            let problems = lint(&directory);
            assert!(problems.is_empty(), "{name}: {problems:?}");
            assert!(create(&directory, Template::List).is_err(), "not empty");
        }
    }

    #[test]
    fn test_bundled_extensions_pass_lint() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions");
        for entry in std::fs::read_dir(root).unwrap() {
            let directory = entry.unwrap().path();
            let errors: Vec<Problem> = lint(&directory)
                .into_iter()
                .filter(|problem| problem.error)
                .collect();
            assert!(errors.is_empty(), "{}: {errors:?}", directory.display());
        }
    }

    #[test]
    fn test_lint_names_what_is_wrong() {
        let folder = tempfile::tempdir().unwrap();
        let directory = folder.path().join("broken");
        create(&directory, Template::List).unwrap();
        std::fs::remove_file(directory.join("commands/broken.js")).unwrap();
        let problems = lint(&directory);
        assert!(
            problems
                .iter()
                .any(|problem| problem.error && problem.message.contains("commands/broken.js")),
            "{problems:?}"
        );
        std::fs::write(directory.join("launcher.json"), "{}").unwrap();
        assert!(lint(&directory).iter().any(|problem| problem.error));
    }
}
