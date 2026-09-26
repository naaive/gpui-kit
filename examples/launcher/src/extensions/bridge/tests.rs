//! Extensions mounted from temporary directories, rendered, and taken apart.
//!
//! Each test writes a small extension, mounts its View on a runtime with the
//! launcher's components, and reads the [`PageModel`] its render produced —
//! the same path `ScriptPage` takes, without a launcher window, so a failure
//! here points at the bridge rather than at the renderer.

use std::{
    cell::RefCell,
    ops::Deref as _,
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui::{Empty, Entity, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, IntoElement as _, ParentElement as _, Render as _, Styled as _};
use gpui_shell::{Capabilities, ScriptView, ShellRuntime, policy::Policy};

use super::{ExtensionContext, HostApi, components, take_page_model};
use crate::{
    extensions::{CommandId, LaunchRequest},
    model::{
        Accessory, ActionEntry, ActionStyle, Control, DetailModel, Effect, FormModel, FormValue,
        FormValues, Image, Layout, ListModel, MetadataValue, PageModel, ToastStyle, Tone,
    },
};

use super::LaunchType;

const EXTENSION: &str = "test.bridge";

/// A mounted command and what it asked the launcher to do.
struct Mounted {
    runtime: Rc<ShellRuntime>,
    view: Entity<ScriptView>,
    cx: VisualTestContext,
    effects: Rc<RefCell<Vec<Effect>>>,
    root: PathBuf,
}

impl Drop for Mounted {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

impl Mounted {
    /// Mounts another extension on the same runtime, replacing this one: a
    /// GPUI app holds one default runtime.
    fn remount(&mut self, name: &str, files: &[(&str, &str)]) {
        std::fs::remove_dir_all(&self.root).ok();
        self.effects.borrow_mut().clear();
        self.root = write_extension(name, files, &self.effects);
        self.view = mount_view(&self.runtime, &self.root, &mut self.cx);
        self.settle();
    }

    /// Renders once and lets what that started finish. Work `init` spawned
    /// resumes when the first render drains the job queue, as it does when
    /// the launcher first shows a page.
    fn settle(&mut self) {
        self.model().ok();
        self.cx.run_until_parked();
    }

    /// Renders the command inside a drawn frame, as the launcher window
    /// does, and takes its page apart.
    fn model(&mut self) -> Result<PageModel, String> {
        let result = Rc::new(RefCell::new(None));
        let probe = self.cx.update(|_, cx| {
            let view = self.view.clone();
            let result = result.clone();
            cx.new(|_| Probe { view, result })
        });
        self.cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(800.), gpui::px(600.)),
            |_, _| gpui::div().size_full().child(probe),
        );
        result.take().expect("the probe rendered")
    }

    fn page(&mut self) -> PageModel {
        self.model().unwrap_or_else(|error| panic!("{error}"))
    }

    fn list(&mut self) -> ListModel {
        match self.page() {
            PageModel::List(list) => list,
            other => panic!("expected a List, got {other:?}"),
        }
    }

    fn detail(&mut self) -> DetailModel {
        match self.page() {
            PageModel::Detail(detail) => detail,
            other => panic!("expected a Detail, got {other:?}"),
        }
    }

    fn form(&mut self) -> FormModel {
        match self.page() {
            PageModel::Form(form) => form,
            other => panic!("expected a Form, got {other:?}"),
        }
    }

    /// Runs a model callback the way the launcher window does, then lets the
    /// script's promises settle.
    fn call(&mut self, body: impl FnOnce(&mut gpui::Window, &mut gpui::App)) {
        self.cx.update(body);
        self.cx.run_until_parked();
    }

    fn effects(&self) -> Vec<Effect> {
        self.effects.borrow().clone()
    }
}

/// Renders a command's view the way `ScriptPage` does and keeps the result.
struct Probe {
    view: Entity<ScriptView>,
    result: Rc<RefCell<Option<Result<PageModel, String>>>>,
}

impl gpui::Render for Probe {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let mut element = self
            .view
            .update(cx, |view, cx| view.render(window, cx).into_any_element());
        let result = match self.view.read(cx).build_error() {
            Some(error) => Err(error.to_owned()),
            None => take_page_model(&mut element),
        };
        self.result.replace(Some(result));
        gpui::Empty
    }
}

/// Writes `files` into a fresh extension directory, and makes the next view's
/// policy import `launcher/api` for it, the way a per-extension host does.
fn write_extension(
    name: &str,
    files: &[(&str, &str)],
    effects: &Rc<RefCell<Vec<Effect>>>,
) -> PathBuf {
    let root = std::env::temp_dir().join(format!("launcher-bridge-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("gpui-shell.json"),
        format!(r#"{{ "id": "{EXTENSION}", "name": "Bridge", "entry": "main.js" }}"#),
    )
    .unwrap();
    for (path, contents) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    let sink = effects.clone();
    let context = ExtensionContext::new(EXTENSION)
        .with_preference("greeting", serde_json::json!("Hello"))
        .with_cache_directory(root.join("cache"))
        .with_effect_sink(move |effect, _| sink.borrow_mut().push(effect));
    context.begin_launch(
        &LaunchRequest::new(CommandId::new(EXTENSION, "main")).with_argument("query", "gpui"),
        LaunchType::UserInitiated,
    );
    gpui_shell::policy::set_default(
        Policy::new()
            .with_application(EXTENSION)
            .with_capabilities(Capabilities::new().storage(true))
            .with_storage_path(root.join("storage.json"))
            .with_host_module(HostApi::module_for(context))
            .unwrap(),
    );
    root
}

fn mount_view(
    runtime: &Rc<ShellRuntime>,
    root: &Path,
    cx: &mut VisualTestContext,
) -> Entity<ScriptView> {
    let view = cx.update(|window, cx| {
        let application = runtime.load_application(root, "main.js").unwrap();
        runtime.mount_application(&application, window, cx).unwrap()
    });
    cx.run_until_parked();
    view
}

fn mount(cx: &mut TestAppContext, name: &str, files: &[(&str, &str)]) -> Mounted {
    let effects = Rc::new(RefCell::new(Vec::new()));
    let root = write_extension(name, files, &effects);
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_shell::init(cx);
    });
    let runtime =
        cx.update(|cx| ShellRuntime::new_with_components(cx, components().unwrap()).unwrap());
    let window = cx.add_window(|_, _| Empty);
    let mut cx = VisualTestContext::from_window(*window.deref(), cx);
    let view = mount_view(&runtime, &root, &mut cx);
    let mut mounted = Mounted {
        runtime,
        view,
        cx,
        effects,
        root,
    };
    mounted.settle();
    mounted
}

fn main_js(source: &str) -> [(&'static str, &str); 1] {
    [("main.js", source)]
}

#[gpui::test]
fn test_list_materializes_every_part(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "list",
        &main_js(
            r#"
import { View } from "gpui-kit";
import {
  Action, ActionPanel, ActionPanelSection, ActionPanelSubmenu, Detail, List, ListDropdown,
  ListDropdownItem, ListItem, ListSection, MetadataLabel,
} from "launcher";

export default class Main extends View {
  init() {
    this.filter = "all";
    this.selected = null;
    this.loaded = 0;
  }

  render() {
    return new List()
      .placeholder(`Filter (${this.filter})`)
      .loading()
      .empty_title("Nothing here")
      .empty_description("Try another filter")
      .showing_detail(true)
      .grid(4)
      .selected_item("b")
      .dropdown(
        new ListDropdown("Scope")
          .value(this.filter)
          .children([new ListDropdownItem("all", "All"), new ListDropdownItem("mine", "Mine")])
          .on_change((value, cx) => { this.filter = value; cx.notify(); }),
      )
      .on_selection_change((id, cx) => { this.selected = id; cx.notify(); })
      .on_load_more((cx) => { this.loaded += 1; cx.notify(); })
      .child(
        new ListItem("a", "Alpha")
          .subtitle(this.selected ?? "none")
          .icon("globe")
          .accessory(`${this.loaded}`)
          .tag("New", "success")
          .accessory_icon("assets/star.png")
          .keyword("first")
          .detail(new Detail(`# Alpha`).child(new MetadataLabel("Kind", "Letter")))
          .actions(
            new ActionPanel()
              .child(new Action("Open").open_url("https://gpui-kit.com"))
              .child(
                new ActionPanelSection("More").children([
                  new Action("Copy").copy("alpha"),
                  new ActionPanelSubmenu("Tag").icon("tag").children([
                    new Action("Red").toast("Red"),
                  ]),
                ]),
              ),
          )
          .action(new Action("Paste").paste("alpha")),
      )
      .child(
        new ListSection("Letters")
          .subtitle("2")
          .children([new ListItem("b", "Beta"), new ListItem("c", "Gamma")]),
      );
  }
}
"#,
        ),
    );

    let list = mounted.list();
    assert_eq!(list.placeholder().map(|p| p.as_ref()), Some("Filter (all)"));
    assert!(
        list.is_loading(),
        "`loading()` without an argument means true"
    );
    assert!(list.is_filtering());
    assert!(list.is_showing_detail());
    assert_eq!(list.layout(), Layout::Grid { columns: 4 });
    assert_eq!(list.selected().map(|id| id.as_str()), Some("b"));
    assert_eq!(list.empty_title().map(|t| t.as_ref()), Some("Nothing here"));
    assert_eq!(
        list.empty_description().map(|t| t.as_ref()),
        Some("Try another filter")
    );
    let dropdown = list.dropdown().expect("the dropdown");
    assert_eq!(dropdown.tooltip().as_ref(), "Scope");
    assert_eq!(dropdown.choices().len(), 2);
    assert_eq!(dropdown.value().map(|v| v.as_ref()), Some("all"));

    assert_eq!(list.sections().len(), 2);
    assert_eq!(list.sections()[1].subtitle().map(|s| s.as_ref()), Some("2"));
    let alpha = list.items().next().unwrap();
    assert_eq!(alpha.image(), Some(&Image::Icon("globe".into())));
    assert_eq!(
        alpha.accessories(),
        [
            Accessory::text("0"),
            Accessory::tag("New", Tone::Success),
            Accessory::image(Image::File("assets/star.png".into())),
        ]
    );
    assert_eq!(alpha.keywords(), ["first"]);
    let detail = alpha.detail().expect("the item's detail");
    assert_eq!(detail.markdown().as_ref(), "# Alpha");
    assert_eq!(detail.metadata().len(), 1);

    // `actions` sets the panel and `action` appends to it.
    let panel = alpha.actions();
    assert_eq!(panel.sections().len(), 3);
    assert_eq!(
        panel.sections()[1].title().map(|t| t.as_ref()),
        Some("More")
    );
    assert!(matches!(
        &panel.sections()[1].entries()[1],
        ActionEntry::Submenu(submenu)
            if submenu.title().as_ref() == "Tag" && submenu.actions().len() == 1
    ));
    assert_eq!(
        panel
            .all_actions()
            .map(|action| action.title().to_string())
            .collect::<Vec<_>>(),
        ["Open", "Copy", "Red", "Paste"]
    );

    // Callbacks reach the script with their argument, and its notify rebuilds.
    let on_selection_change = list.on_selection_change().cloned().unwrap();
    mounted.call(|window, cx| on_selection_change.call("c".into(), window, cx));
    let on_change = list.dropdown().unwrap().on_change().cloned().unwrap();
    mounted.call(|window, cx| on_change.call("mine".into(), window, cx));
    let on_load_more = list.on_load_more().cloned().unwrap();
    mounted.call(|window, cx| on_load_more.run(window, cx));

    let list = mounted.list();
    assert_eq!(
        list.placeholder().map(|p| p.as_ref()),
        Some("Filter (mine)")
    );
    let alpha = list.items().next().unwrap();
    assert_eq!(alpha.subtitle().map(|s| s.as_ref()), Some("c"));
    assert_eq!(alpha.accessories()[0], Accessory::text("1"));
}

#[gpui::test]
fn test_query_change_turns_filtering_off(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "query",
        &main_js(
            r#"
import { View } from "gpui-kit";
import { List, ListItem } from "launcher";

export default class Main extends View {
  init() { this.query = ""; }
  render() {
    return new List()
      .on_query_change((query, cx) => { this.query = query; cx.notify(); })
      .child(new ListItem("q", `Searching for ${this.query}`));
  }
}
"#,
        ),
    );
    let list = mounted.list();
    assert!(
        !list.is_filtering(),
        "a list that searches itself is not filtered"
    );
    let handler = list.on_query_change().cloned().unwrap();
    mounted.call(|window, cx| handler.call("gpui".into(), window, cx));
    assert_eq!(
        mounted.list().items().next().unwrap().title().as_ref(),
        "Searching for gpui"
    );
}

#[gpui::test]
fn test_detail_page_with_metadata(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "detail",
        &main_js(
            r#"
import { View } from "gpui-kit";
import {
  Action, ActionPanel, Detail, MetadataLabel, MetadataLink, MetadataSeparator, MetadataTags,
} from "launcher";

export default class Main extends View {
  render() {
    return new Detail(`# Title\n\nBody`)
      .loading(false)
      .actions(new ActionPanel().child(new Action("Back").pop()))
      .children([
        new MetadataLabel("Author", "Ada"),
        new MetadataLink("Site", "gpui-kit.com", "https://gpui-kit.com"),
        new MetadataSeparator(),
        new MetadataTags("Labels").tag("bug", "danger").tag("ui"),
      ]);
  }
}
"#,
        ),
    );
    let detail = mounted.detail();
    assert_eq!(detail.markdown().as_ref(), "# Title\n\nBody");
    assert!(!detail.is_loading());
    assert!(matches!(
        detail.actions().primary().unwrap().effect(),
        Effect::Pop
    ));
    let metadata = detail.metadata();
    assert_eq!(metadata.len(), 4);
    assert!(matches!(metadata[0].value(), MetadataValue::Text(text) if text.as_ref() == "Ada"));
    assert!(matches!(
        metadata[1].value(),
        MetadataValue::Link { url, .. } if url.as_ref() == "https://gpui-kit.com"
    ));
    assert!(matches!(metadata[2].value(), MetadataValue::Separator));
    let MetadataValue::Tags(tags) = metadata[3].value() else {
        panic!("tags expected");
    };
    assert_eq!(metadata[3].label().as_ref(), "Labels");
    assert_eq!(tags.len(), 2);
    assert_eq!(tags[0].tone(), Tone::Danger);
    assert_eq!(tags[1].tone(), Tone::Neutral);
}

#[gpui::test]
fn test_form_fields_and_submitted_values(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "form",
        &main_js(
            r#"
import { View } from "gpui-kit";
import {
  Action, ActionPanel, Checkbox, DatePicker, Detail, Dropdown, DropdownItem, Form,
  PasswordField, TextArea, TextField,
} from "launcher";

export default class Main extends View {
  init() {
    this.submitted = null;
    this.pinned = false;
  }

  render() {
    if (this.submitted) return new Detail(this.submitted);
    return new Form()
      .actions(
        new ActionPanel().child(
          new Action("Save").submit((values, cx) => {
            this.submitted = JSON.stringify(values);
            cx.notify();
          }),
        ),
      )
      .children([
        new TextField("title", "Title").placeholder("Untitled").default_value("Draft")
          .info("Shown in the list").error("Too short"),
        new TextArea("body", "Body").value("Hello"),
        new PasswordField("token", "Token"),
        new Checkbox("pinned", "Pinned", "Keep at the top").value(this.pinned)
          .on_change((value, cx) => { this.pinned = value; cx.notify(); }),
        new Dropdown("color", "Color").value("red")
          .children([new DropdownItem("red", "Red"), new DropdownItem("blue", "Blue")]),
        new DatePicker("due", "Due").default_value("2026-10-01"),
      ]);
  }
}
"#,
        ),
    );
    let form = mounted.form();
    let fields = form.fields();
    assert_eq!(fields.len(), 6);
    assert_eq!(
        fields[0].control(),
        &Control::Text {
            placeholder: Some("Untitled".into()),
            value: "Draft".into()
        }
    );
    assert_eq!(
        fields[0].info().map(|i| i.as_ref()),
        Some("Shown in the list")
    );
    assert_eq!(fields[0].error().map(|e| e.as_ref()), Some("Too short"));
    assert!(
        matches!(fields[1].control(), Control::TextArea { value, .. } if value.as_ref() == "Hello")
    );
    assert!(matches!(fields[2].control(), Control::Password { .. }));
    assert_eq!(
        fields[3].control(),
        &Control::Checkbox {
            label: "Keep at the top".into(),
            value: false
        }
    );
    assert!(matches!(
        fields[4].control(),
        Control::Dropdown { choices, value } if choices.len() == 2 && value.as_deref() == Some("red")
    ));
    assert_eq!(
        fields[5].control(),
        &Control::Date {
            value: Some("2026-10-01".into())
        }
    );

    // A field's change reaches the script with a typed value.
    let on_change = fields[3].on_change().cloned().unwrap();
    mounted.call(|window, cx| on_change.call(FormValue::Bool(true), window, cx));
    let form = mounted.form();
    assert!(matches!(
        form.fields()[3].control(),
        Control::Checkbox { value: true, .. }
    ));

    // Submitting hands every value over as one plain object.
    let Effect::SubmitForm(submit) = form.actions().primary().unwrap().effect().clone() else {
        panic!("the primary action submits");
    };
    let values = FormValues::new()
        .with("title", FormValue::Text("Groceries".into()))
        .with("pinned", FormValue::Bool(true))
        .with("due", FormValue::Empty);
    mounted.call(|window, cx| submit.call(values, window, cx));
    assert_eq!(
        mounted.detail().markdown().as_ref(),
        r#"{"due":null,"pinned":true,"title":"Groceries"}"#
    );
}

#[gpui::test]
fn test_action_effects_and_modifiers(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "effects-ok",
        &main_js(
            r#"
import { View } from "gpui-kit";
import { Action, List, ListItem } from "launcher";

class Pushed extends View {
  render() { return new List(); }
}

export default class Main extends View {
  init() { this.ran = 0; }
  render() {
    const item = new ListItem("all", `Ran ${this.ran}`);
    return new List().child(
      item
        .action(new Action("Open").open_url("https://gpui-kit.com").icon("globe").shortcut("cmd-o"))
        .action(new Action("Open File").open("/tmp/a.txt"))
        .action(new Action("Reveal").reveal("/tmp/a.txt"))
        .action(new Action("Copy").copy("text"))
        .action(new Action("Paste").paste("text"))
        .action(new Action("Toast").toast("Saved", "success", "All good"))
        .action(new Action("HUD").hud("Copied"))
        .action(new Action("Run").run((cx) => { this.ran += 1; cx.notify(); }))
        .action(new Action("Push").push(() => new Pushed(), "Pushed"))
        .action(new Action("Launch").launch("search").argument("query", "gpui"))
        .action(new Action("Launch Other").launch("other.ext/open"))
        .action(new Action("Pop").pop())
        .action(new Action("Root").pop_to_root())
        .action(new Action("Close").close_window())
        .action(
          new Action("Delete").run(() => {}).destructive().confirm("Delete it?", "It cannot be undone"),
        ),
    );
  }
}
"#,
        ),
    );
    let list = mounted.list();
    let item = list.items().next().unwrap();
    let actions: Vec<_> = item.actions().actions().cloned().collect();
    assert_eq!(actions.len(), 15);
    assert_eq!(actions[0].image(), Some(&Image::Icon("globe".into())));
    assert_eq!(actions[0].shortcut().map(|s| s.as_ref()), Some("cmd-o"));
    let effects: Vec<&Effect> = actions.iter().map(|action| action.effect()).collect();
    assert!(matches!(effects[0], Effect::OpenUrl(url) if url.as_ref() == "https://gpui-kit.com"));
    assert!(matches!(effects[1], Effect::OpenPath(path) if path == Path::new("/tmp/a.txt")));
    assert!(matches!(effects[2], Effect::RevealPath(_)));
    assert!(matches!(effects[3], Effect::Copy(text) if text.as_ref() == "text"));
    assert!(matches!(effects[4], Effect::Paste(_)));
    assert!(matches!(
        effects[5],
        Effect::ShowToast(toast)
            if toast.style() == ToastStyle::Success
                && toast.message().map(|m| m.as_ref()) == Some("All good")
    ));
    assert!(matches!(effects[6], Effect::ShowHud(text) if text.as_ref() == "Copied"));
    assert!(matches!(effects[7], Effect::Run(_)));
    assert!(matches!(effects[8], Effect::Push(_)));
    let Effect::Launch(request) = effects[9] else {
        panic!("launch");
    };
    assert_eq!(request.command(), &CommandId::new(EXTENSION, "search"));
    assert_eq!(
        request.arguments().get("query").map(|q| q.as_ref()),
        Some("gpui")
    );
    assert!(matches!(
        effects[10],
        Effect::Launch(request) if request.command() == &CommandId::new("other.ext", "open")
    ));
    assert!(matches!(effects[11], Effect::Pop));
    assert!(matches!(effects[12], Effect::PopToRoot));
    assert!(matches!(effects[13], Effect::CloseWindow));
    assert_eq!(actions[14].style(), ActionStyle::Destructive);
    let Effect::Confirm(confirmation) = effects[14] else {
        panic!("confirm wraps the effect");
    };
    assert!(confirmation.is_destructive());
    assert_eq!(
        confirmation.message().map(|m| m.as_ref()),
        Some("It cannot be undone")
    );
    assert!(matches!(confirmation.effect(), Effect::Run(_)));

    let Effect::Run(run) = effects[7].clone() else {
        unreachable!()
    };
    mounted.call(|window, cx| run.run(window, cx));
    assert_eq!(
        mounted.list().items().next().unwrap().title().as_ref(),
        "Ran 1"
    );
}

/// Each page renders one misuse; the model says what was wrong with it.
#[gpui::test]
fn test_misused_nodes_explain_themselves(cx: &mut TestAppContext) {
    let cases = [
        (
            r#"new List().child(new ListItem("a", "A").action(new Action("Open")))"#,
            "Action `Open` has no effect",
        ),
        (
            r#"new List().child(new ListItem("a", "A").action(new Action("Both").copy("x").pop()))"#,
            "has two effects, `copy` and `pop`",
        ),
        (
            r#"new List().child(new ListItem("a", "A").action(new Action("Arg").copy("x").argument("q", "1")))"#,
            "only `launch` takes",
        ),
        (
            r#"new List().child(new Detail("x"))"#,
            "List accepts ListSection and ListItem children, not `Detail`",
        ),
        (
            r#"new Form().children([new TextField("a", "A"), new TextField("a", "B")])"#,
            "two fields with the id `a`",
        ),
        (
            r#"new Form().child(new DatePicker("d", "D").value("tomorrow"))"#,
            "is not a YYYY-MM-DD date",
        ),
        (
            r#"new Form().child(new Dropdown("d", "D").value("x").child(new DropdownItem("y", "Y")))"#,
            "no DropdownItem with the value `x`",
        ),
        (r#"new List().grid(0)"#, "whole number of columns"),
        (
            r#"new List().child(new ListItem("", "A"))"#,
            "`id` must not be empty",
        ),
        (
            r#"new List().child(new ListItem("a", "A").tag("x", "loud"))"#,
            "tag(tone) expects",
        ),
        (r#"new List().p_2()"#, "does not take styles"),
    ];
    let mut mounted: Option<Mounted> = None;
    for (ix, (page, expected)) in cases.into_iter().enumerate() {
        let source = format!(
            r#"
import {{ View }} from "gpui-kit";
import {{ Action, Detail, DatePicker, Dropdown, DropdownItem, Form, List, ListItem, TextField }} from "launcher";
export default class Main extends View {{ render() {{ return {page}; }} }}
"#
        );
        let name = format!("misuse-{ix}");
        let mounted = match &mut mounted {
            Some(mounted) => {
                mounted.remount(&name, &main_js(&source));
                mounted
            }
            None => mounted.insert(mount(cx, &name, &main_js(&source))),
        };
        let error = mounted.model().expect_err(page);
        assert!(
            error.contains(expected),
            "{page}\n  expected: {expected}\n  got: {error}"
        );
    }
}

#[gpui::test]
fn test_host_api_answers_for_its_extension(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "api",
        &main_js(
            r#"
import { View } from "gpui-kit";
import { Detail } from "launcher";
import {
  cache_clear, cache_get, cache_remove, cache_set, close_main_window, copy, environment, launch,
  launch_command, open, paste, pop, pop_to_root, show_hud, show_toast, update_command_metadata,
} from "launcher/api";

export default class Main extends View {
  init() {
    const context = launch();
    cache_set("answer", { value: 42, list: [1, "two", null] });
    const cached = cache_get("answer");
    const removed = [cache_remove("answer"), cache_remove("answer")];
    cache_set("gone", 1);
    cache_clear();
    const environment_ = environment();
    this.report = JSON.stringify({
      context,
      cached,
      removed,
      missing: cache_get("gone"),
      development: environment_.development,
      version: typeof environment_.launcher_version,
      appearance: environment_.appearance,
    });
    show_toast({ title: "Loaded", message: "All of it", style: "success", id: "load" });
    show_toast("Legacy", "failure");
    show_hud("Done");
    close_main_window();
    pop();
    pop_to_root();
    open("https://gpui-kit.com");
    copy("copied");
    paste("pasted");
    launch_command("search", { query: "x" });
    update_command_metadata({ subtitle: "3 new" });
  }

  render() {
    return new Detail(this.report);
  }
}
"#,
        ),
    );
    let report: serde_json::Value =
        serde_json::from_str(mounted.detail().markdown()).expect("the report is JSON");
    assert_eq!(
        report,
        serde_json::json!({
            "context": {
                "extension": EXTENSION,
                "command": "main",
                "arguments": { "query": "gpui" },
                "preferences": { "greeting": "Hello" },
                "launch_type": "user_initiated",
            },
            "cached": { "value": 42, "list": [1, "two", null] },
            "removed": [true, false],
            "missing": null,
            "development": false,
            "version": "string",
            "appearance": "light",
        })
    );

    let effects = mounted.effects();
    let names: Vec<String> = effects
        .iter()
        .map(|effect| format!("{effect:?}").split('(').next().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "ShowToast",
            "ShowToast",
            "ShowHud",
            "CloseWindow",
            "Pop",
            "PopToRoot",
            "OpenUrl",
            "Copy",
            "Paste",
            "Launch"
        ]
    );
    assert!(matches!(
        &effects[0],
        Effect::ShowToast(toast) if toast.id().map(|id| id.as_ref()) == Some("load")
    ));
    assert!(matches!(
        &effects[9],
        Effect::Launch(request) if request.command() == &CommandId::new(EXTENSION, "search")
    ));
}

/// `utils.js` is copied beside the command, since `launcher/utils` is served
/// from source only once GPUI Shell can serve host modules that way.
#[gpui::test]
fn test_utils_helpers_drive_a_view(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "utils",
        &[
            ("utils.js", super::utils_module_source()),
            (
                "main.js",
                r#"
import { View } from "gpui-kit";
import { Form, List, ListItem, TextField } from "launcher";
import { FormState, Paginator, Query, frecency_sort } from "./utils.js";

export default class Main extends View {
  init(props, cx) {
    this.repos = new Query(cx, () => Promise.resolve(["gpui", "kit"]), { cache_key: "repos" });
    this.broken = new Query(cx, () => Promise.reject(new Error("offline")), {
      failure_title: "Cannot load",
    });
    this.pages = new Paginator(cx, async (page) => ({
      items: [`p${page}a`, `p${page}b`],
      has_more: page < 1,
    }));
    this.form = new FormState({ title: "" }, { title: FormState.required("Give it a title") });
    this.valid = this.form.validate();
    this.sorted = frecency_sort(
      ["a", "b", "c"],
      (id) => id,
      { c: { count: 5, last: 1000 }, b: { count: 1, last: 1000 } },
      1000,
    );
  }

  render() {
    return new List()
      .loading(this.repos.loading || this.pages.loading)
      .on_load_more((cx) => this.pages.load_more(cx))
      .children([
        ...(this.repos.data ?? []).map((name) => new ListItem(`repo-${name}`, name)),
        ...this.pages.items.map((id) => new ListItem(id, id)),
        new ListItem("error", String(this.broken.error?.message ?? "none")),
        new ListItem("form", `${this.valid}:${this.form.error("title")}`),
        new ListItem("sorted", this.sorted.join("")),
      ]);
  }
}
"#,
            ),
        ],
    );
    let titles = |list: &ListModel| {
        list.items()
            .map(|item| format!("{}={}", item.id().as_str(), item.title()))
            .collect::<Vec<_>>()
    };
    let list = mounted.list();
    assert!(
        !list.is_loading(),
        "resolved loaders have settled: {:?}",
        titles(&list)
    );
    assert_eq!(
        titles(&list),
        [
            "repo-gpui=gpui",
            "repo-kit=kit",
            "p0a=p0a",
            "p0b=p0b",
            "error=offline",
            "form=false:Give it a title",
            "sorted=cba",
        ]
    );
    assert!(
        mounted.effects().iter().any(|effect| matches!(
            effect,
            Effect::ShowToast(toast)
                if toast.style() == ToastStyle::Failure
                    && toast.title().as_ref() == "Cannot load"
                    && toast.message().map(|m| m.as_ref()) == Some("offline")
        )),
        "a failed query shows a failure toast"
    );

    // The next page is appended; the last one says there are no more.
    let load_more = list.on_load_more().cloned().unwrap();
    mounted.call(|window, cx| load_more.run(window, cx));
    mounted.call(|window, cx| load_more.run(window, cx));
    let ids: Vec<String> = mounted
        .list()
        .items()
        .map(|item| item.id().as_str().to_owned())
        .filter(|id| id.starts_with('p'))
        .collect();
    assert_eq!(ids, ["p0a", "p0b", "p1a", "p1b"]);

    // The query cached what it loaded for the next launch.
    let cache = std::fs::read_to_string(mounted.root.join("cache").join("cache.json")).unwrap();
    assert_eq!(cache, r#"{"repos":["gpui","kit"]}"#);
}

#[test]
fn test_write_declarations_describes_every_module() {
    let directory =
        std::env::temp_dir().join(format!("launcher-declarations-{}", std::process::id()));
    std::fs::remove_dir_all(&directory).ok();
    let written = super::write_declarations(&directory).unwrap();
    let names: Vec<String> = written
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for expected in [
        "gpui-kit.d.ts",
        "launcher.d.ts",
        "launcher-api.d.ts",
        "launcher-utils.d.ts",
        "launcher.schema.json",
    ] {
        assert!(names.iter().any(|name| name == expected), "{expected} in {names:?}");
    }
    let components = std::fs::read_to_string(directory.join("gpui-kit.d.ts")).unwrap();
    let shim = std::fs::read_to_string(directory.join("launcher.d.ts")).unwrap();
    assert!(shim.contains("declare module \"launcher\""));
    for export in ["ListDropdown", "MetadataTags", "PasswordField", "ActionPanelSubmenu"] {
        assert!(components.contains(export), "`{export}` is declared");
    }
    let api = std::fs::read_to_string(directory.join("launcher-api.d.ts")).unwrap();
    assert!(api.contains("declare module \"launcher/api\""));
    assert!(api.contains("export function cache_set"));
    let utils = std::fs::read_to_string(directory.join("launcher-utils.d.ts")).unwrap();
    assert!(utils.contains("export class Query<T>"));

    assert!(
        super::write_declarations(&directory)
            .unwrap()
            .iter()
            .all(|path| !path.ends_with("launcher-api.d.ts")),
        "an up-to-date file is not rewritten"
    );
    std::fs::remove_dir_all(&directory).ok();
}
