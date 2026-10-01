//! End-to-end flows across the window, the extension host and the bridge:
//! real extensions, bundled or written to a temporary directory, driven with
//! simulated keys.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, Focusable as _};

use super::{
    LauncherWindow,
    tests::{bundled, depth, open, open_in, page, rows, selected},
};
use crate::{
    extensions::{MemorySecrets, SecretStore as _},
    model::{Item, MetadataValue, PageModel},
    session::Row,
};

/// Extensions kept only for tests: a notes list with a form, and an emoji
/// grid with a dropdown, which exercise the window more than the bundled ones.
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-extensions")
}

/// Writes an extension with `id` into `root/<directory>`: its manifests and
/// `files`, each a module path and its source.
fn write_extension(root: &Path, id: &str, launcher: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("gpui-shell.json"),
        format!(r#"{{ "id": "{id}", "name": "Test", "entry": "main.js" }}"#),
    )
    .unwrap();
    std::fs::write(directory.join("launcher.json"), launcher).unwrap();
    for (path, source) in files {
        std::fs::write(directory.join(path), source).unwrap();
    }
    directory
}

/// A command whose page lists `launch()`'s arguments and preferences as
/// items `argument:<name>=<value>` and `preference:<name>=<value>`.
const ECHO: &str = r#"import { View } from "gpui-kit";
import { List, ListItem } from "launcher";
import { launch } from "launcher/api";

export default class Echo extends View {
  init() {
    const { arguments: args, preferences } = launch();
    this.ids = [
      ...Object.entries(args).map(([name, value]) => `argument:${name}=${value}`),
      ...Object.entries(preferences).map(([name, value]) => `preference:${name}=${value}`),
    ];
  }

  render() {
    return new List().children(this.ids.map((id) => new ListItem(id, id)));
  }
}
"#;

/// The item with `id` in the current page's visible rows.
fn item(launcher: &Entity<LauncherWindow>, id: &str, cx: &mut VisualTestContext) -> Option<Item> {
    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            let (_, rows) = launcher.rows(window, cx);
            rows.rows().iter().find_map(|row| match row {
                Row::Item(item) if item.id().as_str() == id => Some((**item).clone()),
                _ => None,
            })
        })
    })
}

fn loaded_launches(launcher: &Entity<LauncherWindow>, cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| launcher.read(cx).extensions.loaded_launches())
}

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

#[gpui::test]
fn test_a_pushed_page_shows_its_detail_and_pops_back_to_the_selection(cx: &mut TestAppContext) {
    let (launcher, mut cx) = open(cx, &[bundled()]);
    cx.simulate_input("links");
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&launcher, &mut cx), "design");

    // "Show Details" is in a section of the panel, reached by its shortcut.
    cx.simulate_keystrokes("secondary-i");
    assert_eq!(depth(&launcher, &mut cx), 3, "the detail is pushed");
    let PageModel::Detail(detail) = page(&launcher, &mut cx) else {
        panic!("the pushed view renders a Detail");
    };
    assert_eq!(
        detail.markdown().as_ref(),
        "# Design Guides\n\nhttps://gpui-kit.com/docs/design-guides"
    );
    let metadata: Vec<(String, String)> = detail
        .metadata()
        .iter()
        .map(|metadata| {
            let value = match metadata.value() {
                MetadataValue::Text(text) => text.to_string(),
                MetadataValue::Link { url, .. } => url.to_string(),
                other => format!("{other:?}"),
            };
            (metadata.label().to_string(), value)
        })
        .collect();
    assert_eq!(
        metadata,
        [
            ("Title".into(), "Design Guides".into()),
            (
                "Address".into(),
                "https://gpui-kit.com/docs/design-guides".into()
            ),
        ]
    );
    assert_eq!(
        cx.update(|_, cx| launcher.read(cx).navigator.current().page().title(cx)),
        "Design Guides",
        "the page is titled as `push` asked"
    );
    assert_eq!(
        loaded_launches(&launcher, &mut cx),
        1,
        "one launch serves both pages"
    );

    cx.simulate_keystrokes("escape");
    assert_eq!(depth(&launcher, &mut cx), 2);
    assert_eq!(
        selected(&launcher, &mut cx),
        "design",
        "the list keeps its selection"
    );
    assert!(
        cx.update(|window, cx| launcher.read(cx).input.focus_handle(cx).is_focused(window)),
        "the search field has the keyboard again"
    );

    // Back at the root, the command stays loaded for a while, then goes.
    cx.simulate_keystrokes("escape");
    assert_eq!(depth(&launcher, &mut cx), 1);
    cx.run_until_parked();
    assert_eq!(
        loaded_launches(&launcher, &mut cx),
        1,
        "kept for a quick return"
    );
    cx.executor()
        .advance_clock(crate::extensions::KEEP_ALIVE + std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(loaded_launches(&launcher, &mut cx), 0, "and released after");
}

#[gpui::test]
fn test_a_no_view_command_runs_without_a_page_and_is_released(cx: &mut TestAppContext) {
    let (launcher, mut cx) = open(cx, &[bundled()]);
    cx.simulate_input("copy today");
    assert_eq!(
        selected(&launcher, &mut cx),
        "com.gpui-kit.links/copy-date:Command"
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    assert_eq!(depth(&launcher, &mut cx), 1, "no page is pushed");
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert_eq!(clipboard(&mut cx), Some(today));
    assert_eq!(
        loaded_launches(&launcher, &mut cx),
        0,
        "the HUD said it is done, so its view is released"
    );
    assert!(
        cx.update(|_, cx| launcher.read(cx).navigator.current().query().is_empty()),
        "the HUD closed the launcher, which starts over next time"
    );
}

#[gpui::test]
fn test_a_missing_required_argument_is_asked_for_first(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    write_extension(
        root.path(),
        "test.greet",
        r#"{ "commands": [{ "name": "greet", "title": "Greet Someone", "module": "main.js",
            "arguments": [{ "name": "who", "placeholder": "Name", "required": true },
                          { "name": "how", "placeholder": "Greeting" }] }] }"#,
        &[("main.js", ECHO)],
    );
    let (launcher, mut cx) = open(cx, &[root.path().to_path_buf()]);
    cx.simulate_input("greet");
    cx.simulate_keystrokes("enter");
    assert_eq!(depth(&launcher, &mut cx), 2);
    let PageModel::Form(form) = page(&launcher, &mut cx) else {
        panic!("the arguments are asked for in a form");
    };
    assert_eq!(form.fields().len(), 2);
    cx.simulate_keystrokes("tab escape");
    assert_eq!(
        depth(&launcher, &mut cx),
        1,
        "Esc leaves a form from any field"
    );
    cx.simulate_keystrokes("enter");

    // Submitting without the required one keeps the form, with an error.
    cx.simulate_keystrokes("secondary-enter");
    cx.run_until_parked();
    let PageModel::Form(form) = page(&launcher, &mut cx) else {
        panic!("the form stays until the argument is given");
    };
    assert_eq!(
        form.fields()[0].error().map(|e| e.as_ref()),
        Some("Required")
    );

    cx.simulate_input("Ada");
    // The action panel gives the keyboard back to the field that had it.
    cx.simulate_keystrokes("tab secondary-k escape");
    cx.simulate_input("Hi");
    cx.simulate_keystrokes("secondary-enter");
    cx.run_until_parked();
    assert_eq!(
        depth(&launcher, &mut cx),
        2,
        "the form gave way to the command"
    );
    assert_eq!(
        rows(&launcher, &mut cx),
        [">argument:how=Hi", "argument:who=Ada"]
    );
}

fn open_github(cx: &mut VisualTestContext) {
    cx.simulate_input("search github repositories");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
}

/// Whether a loaded launch may reach GitHub's API.
fn reaches_github(launcher: &Entity<LauncherWindow>, cx: &mut VisualTestContext) -> bool {
    cx.update(|_, cx| {
        launcher
            .read(cx)
            .extensions
            .loaded_policies()
            .iter()
            .any(|policy| {
                policy.capabilities().may_request(
                    "https",
                    "api.github.com",
                    None,
                    "GET",
                    "/search/repositories",
                )
            })
    })
}

#[gpui::test]
fn test_permissions_are_asked_before_the_first_run_only(cx: &mut TestAppContext) {
    let (launcher, mut cx) = open(cx, &[bundled()]);
    open_github(&mut cx);
    let PageModel::Detail(detail) = page(&launcher, &mut cx) else {
        panic!("the permission question comes first");
    };
    assert!(detail.markdown().contains("Allow “GitHub” to run?"));
    assert_eq!(loaded_launches(&launcher, &mut cx), 0, "no code ran yet");

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(depth(&launcher, &mut cx), 2, "the question gave way");
    let PageModel::List(list) = page(&launcher, &mut cx) else {
        panic!("Allow opens the command");
    };
    assert_eq!(
        list.placeholder().map(|p| p.as_ref()),
        Some("Search repositories…")
    );
    assert!(
        reaches_github(&launcher, &mut cx),
        "the command runs with the grant"
    );

    cx.simulate_keystrokes("escape");
    open_github(&mut cx);
    assert!(
        matches!(page(&launcher, &mut cx), PageModel::List(_)),
        "a decision is not asked again"
    );
}

#[gpui::test]
fn test_a_refused_permission_still_opens_the_command_without_it(cx: &mut TestAppContext) {
    let (launcher, mut cx) = open(cx, &[bundled()]);
    open_github(&mut cx);
    cx.simulate_keystrokes("secondary-enter");
    cx.run_until_parked();
    assert_eq!(depth(&launcher, &mut cx), 2);
    assert!(
        matches!(page(&launcher, &mut cx), PageModel::List(_)),
        "Don’t Allow still opens the command"
    );
    assert_eq!(loaded_launches(&launcher, &mut cx), 1);
    assert!(!reaches_github(&launcher, &mut cx), "without the grant");

    cx.simulate_keystrokes("escape");
    open_github(&mut cx);
    assert!(
        matches!(page(&launcher, &mut cx), PageModel::List(_)),
        "a refusal is a decision too"
    );
}

#[gpui::test]
fn test_required_preferences_are_asked_for_and_secrets_stay_in_the_keychain(
    cx: &mut TestAppContext,
) {
    let root = tempfile::tempdir().unwrap();
    write_extension(
        root.path(),
        "test.preferences",
        r#"{ "commands": [{ "name": "echo", "title": "Echo Preferences", "module": "main.js" }],
             "preferences": [
               { "name": "greeting", "title": "Greeting", "type": "text", "required": true },
               { "name": "token", "title": "Token", "type": "password", "required": true },
               { "name": "loud", "title": "Loud", "type": "checkbox", "label": "Shout" }
             ] }"#,
        &[("main.js", ECHO)],
    );
    let data = tempfile::tempdir().unwrap();
    let secrets = Rc::new(MemorySecrets::default());
    let (launcher, mut cx) = open_in(
        cx,
        &[root.path().to_path_buf()],
        data.path(),
        secrets.clone(),
    );
    cx.simulate_input("echo preferences");
    cx.simulate_keystrokes("enter");
    let PageModel::Form(form) = page(&launcher, &mut cx) else {
        panic!("the preferences are asked for first");
    };
    assert_eq!(form.fields().len(), 3);
    assert_eq!(loaded_launches(&launcher, &mut cx), 0, "no code ran yet");

    cx.simulate_input("Hello");
    cx.simulate_keystrokes("tab");
    cx.simulate_input("s3cret");
    cx.simulate_keystrokes("secondary-enter");
    cx.run_until_parked();

    assert_eq!(
        depth(&launcher, &mut cx),
        2,
        "the form gave way to the command"
    );
    assert_eq!(
        rows(&launcher, &mut cx),
        [
            ">preference:greeting=Hello",
            "preference:loud=false",
            "preference:token=s3cret"
        ]
    );
    let file = std::fs::read_to_string(data.path().join("preferences.json")).unwrap();
    assert!(file.contains("Hello"), "{file}");
    assert!(
        !file.contains("s3cret"),
        "a password never reaches the file"
    );
    assert_eq!(
        secrets.read("test.preferences/token").unwrap().as_deref(),
        Some("s3cret")
    );

    // Saved: the next launch goes straight to the command.
    cx.simulate_keystrokes("escape");
    cx.simulate_input("echo preferences");
    cx.simulate_keystrokes("enter");
    assert!(matches!(page(&launcher, &mut cx), PageModel::List(_)));
}

#[gpui::test]
fn test_a_fallback_command_receives_the_query(cx: &mut TestAppContext) {
    let (launcher, mut cx) = open(cx, &[bundled()]);
    cx.simulate_input("qqzzx widgets");
    assert_eq!(
        rows(&launcher, &mut cx),
        [
            "# Use “qqzzx widgets” with…",
            ">fallback/google:Web",
            "fallback/com.gpui-kit.github/search-repositories:Command",
            "fallback/com.gpui-kit.links/search-docs:Command",
        ]
    );
    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://duckduckgo.com/?q=site%3Agpui-kit.com%20qqzzx%20widgets"),
        "the no-view command searched for the query"
    );
    assert_eq!(depth(&launcher, &mut cx), 1);
    assert_eq!(
        loaded_launches(&launcher, &mut cx),
        0,
        "closing the window finished it"
    );
}

#[gpui::test]
fn test_a_command_changes_its_subtitle_in_the_root_search(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    write_extension(
        root.path(),
        "test.inbox",
        r#"{ "commands": [{ "name": "inbox", "title": "Open Inbox", "subtitle": "Mail",
                            "module": "main.js" }] }"#,
        &[(
            "main.js",
            r#"import { View } from "gpui-kit";
import { Action, List, ListItem } from "launcher";
import { update_command_metadata } from "launcher/api";

export default class Inbox extends View {
  init() {
    update_command_metadata({ subtitle: "2 unread" });
  }

  render() {
    return new List().children([
      new ListItem("read", "Mark All as Read").action(
        new Action("Mark All as Read").run(() => update_command_metadata({ subtitle: null })),
      ),
      new ListItem("fetch", "Fetch Mail").action(
        new Action("Fetch Mail").run(() => update_command_metadata({ subtitle: "5 unread" })),
      ),
    ]);
  }
}
"#,
        )],
    );
    let (launcher, mut cx) = open(cx, &[root.path().to_path_buf()]);
    let subtitle = |cx: &mut VisualTestContext| {
        item(&launcher, "test.inbox/inbox", cx)
            .and_then(|item| item.subtitle().cloned())
            .map(|subtitle| subtitle.to_string())
    };
    assert_eq!(subtitle(&mut cx).as_deref(), Some("Mail"));

    cx.simulate_input("inbox");
    cx.simulate_keystrokes("enter escape escape");
    assert_eq!(depth(&launcher, &mut cx), 1);
    assert_eq!(subtitle(&mut cx).as_deref(), Some("2 unread"));

    cx.simulate_input("inbox");
    cx.simulate_keystrokes("enter enter escape escape");
    assert_eq!(
        subtitle(&mut cx).as_deref(),
        Some("Mail"),
        "`null` restores the manifest's subtitle"
    );

    cx.simulate_input("inbox");
    cx.simulate_keystrokes("enter down enter escape escape");
    assert_eq!(subtitle(&mut cx).as_deref(), Some("5 unread"));

    // A window opened later, as every summon does where a window cannot be
    // hidden, shows it too.
    let (catalog, extensions) = cx.update(|_, cx| {
        let launcher = launcher.read(cx);
        (launcher.catalog.clone(), launcher.extensions.clone())
    });
    let (_, reopened) = cx
        .update(|_, cx| {
            gpui_kit::open_window(Default::default(), cx, move |window, cx| {
                cx.new(|cx| LauncherWindow::new(catalog, extensions, window, cx))
            })
        })
        .unwrap();
    assert_eq!(
        item(&reopened, "test.inbox/inbox", &mut cx)
            .and_then(|item| item.subtitle().cloned())
            .as_deref(),
        Some("5 unread")
    );
}

#[gpui::test]
fn test_launcher_utils_loads_through_the_extension_policy(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    write_extension(
        root.path(),
        "test.query",
        r#"{ "commands": [{ "name": "load", "title": "Load Things", "module": "main.js" }] }"#,
        &[(
            "main.js",
            r#"import { View } from "gpui-kit";
import { List, ListItem } from "launcher";
import { Query } from "launcher/utils";

export default class Load extends View {
  init(props, cx) {
    this.things = new Query(cx, () => Promise.resolve(["alpha", "beta"]), { initial: [] });
  }

  render() {
    return new List()
      .loading(this.things.loading)
      .children(this.things.data.map((id) => new ListItem(id, id)));
  }
}
"#,
        )],
    );
    let (launcher, mut cx) = open(cx, &[root.path().to_path_buf()]);
    cx.simulate_input("load things");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(rows(&launcher, &mut cx), [">alpha", "beta"]);
    assert!(!page(&launcher, &mut cx).is_loading());
}

#[gpui::test]
fn test_an_extension_installed_from_git_is_listed(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let work = write_extension(
        root.path(),
        "test.installed",
        r#"{ "commands": [{ "name": "hello", "title": "Say Hello", "module": "main.js" }] }"#,
        &[("main.js", ECHO)],
    );
    let git = |directory: &Path, arguments: &[&str]| {
        let status = std::process::Command::new("git")
            .args(arguments)
            .current_dir(directory)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    };
    let remote = root.path().join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare", "--quiet"]);
    git(&work, &["init", "--quiet"]);
    git(&work, &["add", "."]);
    git(
        &work,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--quiet",
            "-m",
            "first",
        ],
    );
    git(
        &work,
        &["push", "--quiet", remote.to_str().unwrap(), "HEAD"],
    );

    let data = tempfile::tempdir().unwrap();
    let directory = crate::extensions::DataDirectory::new(data.path());
    let installed =
        crate::extensions::install::install(&directory, remote.to_str().unwrap()).unwrap();
    assert_eq!(installed.id(), "test.installed");

    let (launcher, mut cx) = open_in(
        cx,
        &[directory.extensions_dir()],
        data.path(),
        Rc::new(MemorySecrets::default()),
    );
    cx.simulate_input("say hello");
    assert_eq!(selected(&launcher, &mut cx), "test.installed/hello:Command");
    cx.simulate_keystrokes("enter");
    assert!(matches!(page(&launcher, &mut cx), PageModel::List(_)));
}

#[gpui::test]
fn test_a_note_created_from_the_list_is_there_when_the_list_opens_again(cx: &mut TestAppContext) {
    let (launcher, mut cx) = open(cx, &[fixtures()]);
    let notes = |cx: &mut VisualTestContext| {
        cx.simulate_input("search notes");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let PageModel::List(list) = page(&launcher, cx) else {
            panic!("notes are listed");
        };
        list.sections()
            .iter()
            .flat_map(|section| section.items())
            .map(|item| item.title().to_string())
            .collect::<Vec<_>>()
    };
    let write = |title: &str, cx: &mut VisualTestContext| {
        cx.simulate_input(title);
        cx.simulate_keystrokes("secondary-enter");
        cx.run_until_parked();
    };

    cx.simulate_input("create note");
    cx.simulate_keystrokes("enter");
    write("First", &mut cx);
    assert_eq!(depth(&launcher, &mut cx), 1, "saving hid the launcher");
    assert_eq!(notes(&mut cx), ["First"]);

    // "Create Note" from the list opens the other command while the list's
    // own is still open; its note must not be lost to the list's copy.
    cx.simulate_keystrokes("secondary-n");
    assert_eq!(depth(&launcher, &mut cx), 3);
    write("Second", &mut cx);
    assert_eq!(depth(&launcher, &mut cx), 1);
    assert_eq!(notes(&mut cx), ["Second", "First"]);

    // Deleting in the list keeps the note the other command wrote.
    cx.simulate_keystrokes("down ctrl-x");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        depth(&launcher, &mut cx),
        2,
        "the list stays after deleting"
    );
    cx.simulate_keystrokes("escape escape");
    assert_eq!(notes(&mut cx), ["Second"]);
}

#[gpui::test]
fn test_editing_a_note_returns_to_the_list_with_the_change(cx: &mut TestAppContext) {
    let (launcher, mut cx) = open(cx, &[fixtures()]);
    cx.simulate_input("create note");
    cx.simulate_keystrokes("enter tab escape");
    assert_eq!(
        depth(&launcher, &mut cx),
        1,
        "Esc leaves a form from its text area"
    );
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Groceries");
    cx.simulate_keystrokes("tab");
    cx.simulate_input("Milk");
    cx.simulate_keystrokes("secondary-enter");
    cx.run_until_parked();
    assert_eq!(
        depth(&launcher, &mut cx),
        1,
        "Cmd/Ctrl-Enter submits from the text area"
    );

    cx.simulate_input("search notes");
    cx.simulate_keystrokes("enter");
    let selection = selected(&launcher, &mut cx);
    let (note, _) = selection.split_once(':').expect("a note shows its date");
    cx.simulate_keystrokes("secondary-e");
    assert_eq!(depth(&launcher, &mut cx), 3, "the form is pushed");
    let PageModel::Form(form) = page(&launcher, &mut cx) else {
        panic!("Edit Note pushes a form");
    };
    assert_eq!(
        form.fields()[0].control().initial_value(),
        crate::model::FormValue::Text("Groceries".into())
    );

    cx.simulate_input(" and more");
    cx.simulate_keystrokes("secondary-enter");
    cx.run_until_parked();
    assert_eq!(depth(&launcher, &mut cx), 2, "saving returns to the list");
    assert_eq!(
        selected(&launcher, &mut cx),
        selection,
        "the note stays selected"
    );
    assert_eq!(
        item(&launcher, note, &mut cx).map(|item| item.title().to_string()),
        Some("Groceries and more".into()),
        "the list shows the change"
    );
    assert!(
        cx.update(|window, cx| launcher.read(cx).input.focus_handle(cx).is_focused(window)),
        "the search field has the keyboard again"
    );
}

#[gpui::test]
fn test_a_list_dropdown_narrows_an_extension_grid(cx: &mut TestAppContext) {
    let (launcher, mut cx) = open(cx, &[fixtures()]);
    cx.simulate_input("search emoji");
    cx.simulate_keystrokes("enter");
    let headers = |cx: &mut VisualTestContext| {
        rows(&launcher, cx)
            .into_iter()
            .filter(|row| row.starts_with('#'))
            .collect::<Vec<_>>()
    };
    assert_eq!(headers(&mut cx).len(), 5, "every category at first");
    assert!(
        cx.update(|_, cx| !launcher.read(cx).dropdowns.is_empty()),
        "the dropdown is beside the search field"
    );

    cx.update(|window, cx| {
        launcher.update(cx, |launcher, cx| {
            let entry = launcher.navigator.current().id();
            launcher.dropdown_changed(entry, "nature".into(), window, cx)
        })
    });
    cx.run_until_parked();
    assert_eq!(headers(&mut cx), ["# Nature"]);
    assert!(
        cx.update(|window, cx| launcher.read(cx).input.focus_handle(cx).is_focused(window)),
        "typing goes back to the search field"
    );

    // The launcher filters the grid by the search text as well.
    let before = rows(&launcher, &mut cx).len();
    cx.simulate_input("pet");
    let after = rows(&launcher, &mut cx);
    assert!(after.len() < before && after.len() > 1, "{after:?}");
}

#[gpui::test]
fn test_a_command_kept_loaded_opens_in_a_new_window(cx: &mut TestAppContext) {
    let (launcher, mut cx) = open(cx, &[bundled()]);
    cx.simulate_input("checklist");
    cx.simulate_keystrokes("enter down enter");
    assert_eq!(rows(&launcher, &mut cx)[1], ">run:Done");

    // Where a window cannot be hidden, hiding closes it and the next summon
    // opens another; the command is still loaded when that happens.
    let (catalog, extensions) = cx.update(|_, cx| {
        let launcher = launcher.read(cx);
        (launcher.catalog.clone(), launcher.extensions.clone())
    });
    drop(launcher);
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
    assert_eq!(extensions.loaded_launches(), 1);

    let (handle, reopened) = TestAppContext::update(&mut cx, |cx| {
        gpui_kit::open_window(Default::default(), cx, move |window, cx| {
            cx.new(|cx| LauncherWindow::new(catalog, extensions, window, cx))
        })
    })
    .unwrap();
    let mut cx = VisualTestContext::from_window(handle, &mut cx);
    cx.run_until_parked();
    cx.simulate_input("checklist");
    cx.simulate_keystrokes("enter");
    assert_eq!(
        rows(&reopened, &mut cx)[..2],
        [">install:Done", "run:Done"],
        "the command is as it was left"
    );
    cx.simulate_keystrokes("down enter");
    assert_eq!(rows(&reopened, &mut cx)[1], ">run", "and still works");
}
