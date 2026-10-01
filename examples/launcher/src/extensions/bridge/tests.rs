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
use gpui_kit::{AppContext as _, IntoElement as _, ParentElement as _, Styled as _};
use gpui_shell::{Capabilities, ScriptView, ShellRuntime, plugin::PluginManifest, policy::Policy};

use super::{ExtensionContext, HostApi, components, take_menu_bar, take_page_model};
use crate::{
    extensions::{Catalog, CommandId, LaunchRequest},
    model::{
        Accessory, ActionEntry, ActionStyle, Control, DetailModel, Effect, FormModel, FormValue,
        FormValues, Image, Item, Layout, ListModel, MenuBarEntry, MenuBarModel, MetadataValue,
        PageModel, ToastStyle, Tone,
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

    /// Renders a `menu-bar` command and takes its menu apart, as the tray
    /// does.
    fn menu_bar(&mut self) -> Result<MenuBarModel, String> {
        let result = Rc::new(RefCell::new(None));
        let probe = self.cx.update(|_, cx| {
            let view = self.view.clone();
            let result = result.clone();
            cx.new(|_| MenuProbe { view, result })
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

/// Renders a command's view the way the tray does and keeps the menu.
struct MenuProbe {
    view: Entity<ScriptView>,
    result: Rc<RefCell<Option<Result<MenuBarModel, String>>>>,
}

impl gpui::Render for MenuProbe {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let mut element = self
            .view
            .update(cx, |view, cx| view.render(window, cx).into_any_element());
        self.result.replace(Some(take_menu_bar(&mut element)));
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

/// The actions an extension gets from the launcher's own features, and the
/// item options for icons and dates.
#[gpui::test]
fn test_launcher_actions_and_item_options(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "launcher-actions",
        &main_js(
            r#"
import { View } from "gpui-kit";
import { Action, List, ListItem } from "launcher";

export default class Main extends View {
  init() { this.picked = "none"; }
  render() {
    return new List().children([
      new ListItem("file", this.picked)
        .file_icon("C:/Windows/notepad.exe")
        .accessory_date("2020-01-02")
        .action(new Action("Open With").open_with("/tmp/a.txt", "notepad"))
        .action(new Action("Trash").trash(["/tmp/a.txt", "/tmp/b.txt"]))
        .action(new Action("Quick Look").quick_look("/tmp/a.png"))
        .action(new Action("Save Link").create_quicklink("Docs", "https://gpui-kit.com/{argument}"))
        .action(new Action("Save Snippet").create_snippet("Hello"))
        .action(new Action("Snooze").pick_date((date, cx) => { this.picked = date; cx.notify(); }, true)),
      new ListItem("avatar", "Avatar").icon("https://example.com/a.png").icon_mask("circle"),
      new ListItem("tinted", "Tinted").icon("star").icon_tone("warning"),
    ]);
  }
}
"#,
        ),
    );
    let list = mounted.list();
    let items: Vec<&Item> = list.items().collect();
    assert_eq!(
        items[0].image(),
        Some(&Image::FileIcon("C:/Windows/notepad.exe".into()))
    );
    let date = &items[0].accessories()[0];
    assert_eq!(date.label().map(|label| label.as_ref()), Some("2020-01-02"));
    assert_eq!(
        date.tooltip().map(|tooltip| tooltip.as_ref()),
        Some("2020-01-02")
    );
    assert_eq!(
        items[1].image(),
        Some(&Image::Circle(Box::new(Image::Url(
            "https://example.com/a.png".into()
        ))))
    );
    assert_eq!(
        items[2].image(),
        Some(&Image::TintedIcon("star".into(), Tone::Warning))
    );

    let effects: Vec<Effect> = items[0]
        .actions()
        .actions()
        .map(|action| action.effect().clone())
        .collect();
    assert!(matches!(
        &effects[0],
        Effect::OpenWith { target, application }
            if target.as_ref() == "/tmp/a.txt" && application.as_ref() == "notepad"
    ));
    assert!(matches!(&effects[1], Effect::Trash(paths) if paths.len() == 2));
    assert!(matches!(&effects[2], Effect::QuickLook(path) if path == Path::new("/tmp/a.png")));
    assert!(matches!(
        &effects[3],
        Effect::CreateQuicklink { name, link }
            if name.as_ref() == "Docs" && link.as_ref() == "https://gpui-kit.com/{argument}"
    ));
    assert!(matches!(
        &effects[4],
        Effect::CreateSnippet { name, text } if name.is_empty() && text.as_ref() == "Hello"
    ));

    // Picking a date pushes a form with a date and time; choosing calls back.
    let Effect::Push(push) = effects[5].clone() else {
        panic!("pick_date pushes a page");
    };
    let page = mounted
        .cx
        .update(|window, cx| push.build(window, cx))
        .unwrap();
    let PageModel::Form(form) = mounted.cx.update(|window, cx| page.model(window, cx)) else {
        panic!("a form");
    };
    assert!(matches!(
        form.fields()[0].control(),
        Control::DateTime { value: None }
    ));
    let Effect::SubmitForm(choose) = form.actions().primary().unwrap().effect().clone() else {
        panic!("choosing submits");
    };
    mounted.call(|window, cx| {
        choose.call(
            FormValues::new().with("date", FormValue::Text("2026-10-02T09:30".into())),
            window,
            cx,
        )
    });
    assert_eq!(
        mounted.list().items().next().unwrap().title().as_ref(),
        "2026-10-02T09:30"
    );
}

/// Choosing files, tags and a time; arranging fields; and what a submit
/// handler receives for them.
#[gpui::test]
fn test_form_pickers_and_arrangement(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "form-pickers",
        &main_js(
            r#"
import { View } from "gpui-kit";
import {
  Action, ActionPanel, DatePicker, FilePicker, Form, FormDescription, FormSeparator, TagPicker,
  TagPickerItem,
} from "launcher";

export default class Main extends View {
  init() { this.submitted = null; }
  render() {
    return new Form().children([
      new FormDescription("Choose what to back up."),
      new FilePicker("folders", "Folders").directories().multiple().value(["/tmp/a", "/tmp/b"]),
      new FormSeparator(),
      new TagPicker("labels", "Labels").value(["work"]).children([
        new TagPickerItem("work", "Work"),
        new TagPickerItem("home", "Home"),
      ]),
      new DatePicker("at", "At").include_time().value("2026-10-01T08:15"),
      new FormDescription("Note", this.submitted ?? "not yet"),
    ]).actions(new ActionPanel().child(new Action("Save").submit((values, cx) => {
      this.submitted = JSON.stringify(values);
      cx.notify();
    })));
  }
}
"#,
        ),
    );
    let form = mounted.form();
    let controls: Vec<&Control> = form.fields().iter().map(|field| field.control()).collect();
    assert!(
        matches!(controls[0], Control::Description { text } if text.as_ref() == "Choose what to back up.")
    );
    assert!(matches!(
        controls[1],
        Control::Files { value, directories: true, multiple: true } if value.len() == 2
    ));
    assert!(matches!(controls[2], Control::Separator));
    assert!(matches!(
        controls[3],
        Control::Tags { choices, value } if choices.len() == 2 && value.len() == 1
    ));
    assert!(matches!(
        controls[4],
        Control::DateTime { value: Some(value) } if value.as_ref() == "2026-10-01T08:15"
    ));
    let ids: Vec<&str> = form
        .fields()
        .iter()
        .map(|field| field.id().as_ref())
        .collect();
    assert_eq!(ids, ["#0", "folders", "#2", "labels", "at", "#5"]);

    let Effect::SubmitForm(submit) = form.actions().primary().unwrap().effect().clone() else {
        panic!("submit");
    };
    let values = FormValues::new()
        .with(
            "folders",
            FormValue::List(vec!["/tmp/a".into(), "/tmp/b".into()]),
        )
        .with("labels", FormValue::List(vec!["work".into()]))
        .with("at", FormValue::Text("2026-10-01T08:15".into()));
    mounted.call(|window, cx| submit.call(values, window, cx));
    let form = mounted.form();
    let Control::Description { text } = form.fields()[5].control() else {
        panic!("description");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text).unwrap(),
        serde_json::json!({
            "at": "2026-10-01T08:15",
            "folders": ["/tmp/a", "/tmp/b"],
            "labels": ["work"],
        })
    );

    mounted.remount(
        "form-pickers-bad",
        &main_js(
            r#"
import { View } from "gpui-kit";
import { Form, TagPicker, TagPickerItem } from "launcher";
export default class Main extends View {
  render() {
    return new Form().child(
      new TagPicker("labels", "Labels").value(["missing"]).child(new TagPickerItem("work", "Work")),
    );
  }
}
"#,
        ),
    );
    let error = mounted.model().unwrap_err();
    assert!(
        error.contains("no TagPickerItem with the value `missing`"),
        "{error}"
    );
}

/// A `menu-bar` command's render, taken apart as the tray reads it.
#[gpui::test]
fn test_menu_bar_extra(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "menu-bar",
        &main_js(
            r#"
import { View } from "gpui-kit";
import {
  Action, MenuBarExtra, MenuBarItem, MenuBarSection, MenuBarSeparator, MenuBarSubmenu,
} from "launcher";

export default class Main extends View {
  init() { this.count = 3; }
  render() {
    return new MenuBarExtra().icon("bell").title(`${this.count}`).tooltip("Unread").children([
      new MenuBarItem("Mark All Read").action(new Action("Read").run((cx) => {
        this.count = 0;
        cx.notify();
      })),
      new MenuBarSeparator(),
      new MenuBarSection("Inbox").children([
        new MenuBarItem("Hello").subtitle("Ada").checked(true),
        new MenuBarSubmenu("More").children([
          new MenuBarItem("Archive"),
          new MenuBarSeparator(),
          new MenuBarItem("Delete"),
        ]),
      ]),
    ]);
  }
}
"#,
        ),
    );
    let menu = mounted.menu_bar().unwrap();
    assert_eq!(menu.icon(), Some(&Image::Icon("bell".into())));
    assert_eq!(menu.title().map(|title| title.as_ref()), Some("3"));
    assert_eq!(menu.sections().len(), 2);
    assert_eq!(
        menu.sections()[1].title().map(|title| title.as_ref()),
        Some("Inbox")
    );
    let MenuBarEntry::Item(hello) = &menu.sections()[1].entries()[0] else {
        panic!("an item");
    };
    assert_eq!(
        hello.subtitle().map(|subtitle| subtitle.as_ref()),
        Some("Ada")
    );
    assert_eq!(hello.checked(), Some(true));
    let MenuBarEntry::Submenu { entries, .. } = &menu.sections()[1].entries()[1] else {
        panic!("a submenu");
    };
    assert!(matches!(entries[1], MenuBarEntry::Separator));

    let MenuBarEntry::Item(read) = &menu.sections()[0].entries()[0] else {
        panic!("an item");
    };
    let Effect::Run(run) = read.action().unwrap().effect().clone() else {
        panic!("run");
    };
    mounted.call(|window, cx| run.run(window, cx));
    assert_eq!(
        mounted
            .menu_bar()
            .unwrap()
            .title()
            .map(|title| title.as_ref()),
        Some("0")
    );
    assert!(mounted.model().is_err(), "a MenuBarExtra is not a page");
}

/// The functions that answer later: a confirmation, a toast's button, and
/// requests the launcher refuses for the extension.
#[gpui::test]
fn test_host_api_promises_and_launch_context(cx: &mut TestAppContext) {
    let mut mounted = mount(
        cx,
        "api-async",
        &main_js(
            r#"
import { View } from "gpui-kit";
import { Detail } from "launcher";
import {
  confirm_alert, environment, launch_command, oauth_authorize, open, selected_text, show_toast,
} from "launcher/api";

export default class Main extends View {
  init(_props, cx) {
    this.events = [];
    const note = (event) => { this.events.push(event); cx.notify(); };
    confirm_alert({ title: "Delete it?", message: "Gone for good", primary_action: "Delete", destructive: true })
      .then((confirmed) => note(`confirmed ${confirmed}`));
    show_toast({ title: "Deleted", primary_action: "Undo" }).then((choice) => note(`toast ${choice}`));
    show_toast("Plain").then((choice) => note(`plain ${choice}`));
    oauth_authorize({
      provider: "github",
      authorize_url: "https://github.com/login/oauth/authorize",
      token_url: "https://github.com/login/oauth/access_token",
      client_id: "abc",
    }).catch((error) => note(`oauth ${String(error).includes("does not allow POST")}`));
    launch_command("detail", {}, { id: 7, tags: ["a"] });
    open("/tmp/a.txt", "notepad");
    note(`selected ${selected_text()}`);
    note(`paths ${typeof environment().assets_path}`);
  }
  render() {
    return new Detail(this.events.join("\n"));
  }
}
"#,
        ),
    );
    let events = |mounted: &mut Mounted| -> Vec<String> {
        mounted
            .detail()
            .markdown()
            .lines()
            .map(str::to_owned)
            .collect()
    };
    let initial = events(&mut mounted);
    assert!(initial.contains(&"plain null".to_owned()), "{initial:?}");
    assert!(initial.contains(&"oauth true".to_owned()), "{initial:?}");
    assert!(initial.contains(&"paths string".to_owned()), "{initial:?}");
    assert!(initial.iter().any(|event| event.starts_with("selected")));

    let effects = mounted.effects();
    let Some(Effect::Confirm(confirmation)) = effects.first().cloned() else {
        panic!("a confirmation first: {effects:?}");
    };
    assert_eq!(confirmation.confirm_title().as_ref(), "Delete");
    assert!(confirmation.is_destructive());
    assert_eq!(
        confirmation.message().map(|message| message.as_ref()),
        Some("Gone for good")
    );
    let Effect::ShowToast(toast) = effects[1].clone() else {
        panic!("a toast with a button");
    };
    let (title, undo) = toast.action().cloned().unwrap();
    assert_eq!(title.as_ref(), "Undo");
    assert!(matches!(
        &effects[3],
        Effect::Launch(request)
            if request.context() == Some(&serde_json::json!({ "id": 7, "tags": ["a"] }))
    ));
    assert!(matches!(
        &effects[4],
        Effect::OpenWith { application, .. } if application.as_ref() == "notepad"
    ));

    let Effect::Run(confirm) = confirmation.effect().clone() else {
        panic!("confirming runs");
    };
    mounted.call(|window, cx| confirm.run(window, cx));
    mounted.call(|window, cx| undo.run(window, cx));
    mounted.settle();
    let events = events(&mut mounted);
    assert!(events.contains(&"confirmed true".to_owned()), "{events:?}");
    assert!(events.contains(&"toast primary".to_owned()), "{events:?}");
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
                "context": null,
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
        assert!(
            names.iter().any(|name| name == expected),
            "{expected} in {names:?}"
        );
    }
    let components = std::fs::read_to_string(directory.join("gpui-kit.d.ts")).unwrap();
    let shim = std::fs::read_to_string(directory.join("launcher.d.ts")).unwrap();
    assert!(shim.contains("declare module \"launcher\""));
    for export in [
        "ListDropdown",
        "MetadataTags",
        "PasswordField",
        "ActionPanelSubmenu",
    ] {
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

// MARK: Bundled extensions

/// Mounts a bundled command the way the launcher will: with its extension's
/// id and the capabilities its `gpui-shell.json` declares, its preferences'
/// defaults, and `arguments`. Storage and cache live in a temporary directory
/// that outlives remounts, so a command sees what an earlier one saved.
fn open_bundled(
    runtime: &Rc<ShellRuntime>,
    cx: &mut VisualTestContext,
    effects: &Rc<RefCell<Vec<Effect>>>,
    data: &Path,
    command: &str,
    arguments: &[(&str, &str)],
) -> Entity<ScriptView> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions");
    open_in(&root, false, runtime, cx, effects, data, command, arguments)
}

/// Mounts `command` of an extension found under `root`. Offline, the
/// extension gets everything it asks for but the network and the programs
/// it runs, as on a computer without them, and a required preference
/// without a default is filled with `test`.
#[allow(clippy::too_many_arguments)]
fn open_in(
    root: &Path,
    offline: bool,
    runtime: &Rc<ShellRuntime>,
    cx: &mut VisualTestContext,
    effects: &Rc<RefCell<Vec<Effect>>>,
    data: &Path,
    command: &str,
    arguments: &[(&str, &str)],
) -> Entity<ScriptView> {
    let (extension_id, name) = command.split_once('/').expect("`extension/command`");
    let catalog = Catalog::discover(&[root.to_path_buf()]);
    let (extension, command) = catalog
        .command(&CommandId::new(extension_id, name))
        .unwrap_or_else(|| panic!("`{extension_id}/{name}` is in {}", root.display()));
    let manifest = PluginManifest::read(extension.root()).unwrap();
    let preferences: serde_json::Map<String, serde_json::Value> = extension
        .preferences()
        .iter()
        .chain(command.preferences())
        .filter_map(|preference| {
            let value = preference
                .default
                .clone()
                .or_else(|| (offline && preference.required).then(|| serde_json::json!("test")))?;
            Some((preference.name.clone(), value))
        })
        .collect();
    let capabilities = match offline {
        false => manifest.capabilities(extension.root(), data),
        true => {
            let requested =
                super::super::permissions::RequestedCapabilities::read(extension.root()).unwrap();
            let approved = requested
                .items()
                .iter()
                .map(|item| item.key().to_owned())
                .filter(|key| !key.starts_with("network.") && !key.starts_with("fs.execute:"))
                .collect();
            requested.grant(&approved, extension.root(), data)
        }
    };
    let request = arguments.iter().fold(
        LaunchRequest::new(command.id().clone()),
        |request, (name, value)| request.with_argument(*name, *value),
    );

    std::fs::create_dir_all(data).unwrap();
    let sink = effects.clone();
    let context = ExtensionContext::new(extension.id().clone())
        .with_preferences(preferences)
        .with_cache_directory(data.join("cache"))
        .with_paths(extension.root(), data)
        .with_capabilities(capabilities.clone())
        .with_secrets(Rc::new(crate::extensions::MemorySecrets::default()))
        .with_effect_sink(move |effect, _| sink.borrow_mut().push(effect));
    context.begin_launch(&request, LaunchType::UserInitiated);
    gpui_shell::policy::set_default(
        Policy::new()
            .with_application(extension.id())
            .with_capabilities(capabilities)
            .with_host_module(
                gpui_shell::HostModule::source(super::UTILS_MODULE, super::utils_module_source())
                    .declarations(super::utils_declarations()),
            )
            .unwrap()
            .with_storage_path(data.join("storage.json"))
            .with_host_module(HostApi::module_for(context))
            .unwrap(),
    );
    let view = cx.update(|window, cx| {
        let application = runtime
            .load_application(extension.root(), command.module())
            .unwrap();
        runtime.mount_application(&application, window, cx).unwrap()
    });
    cx.run_until_parked();
    view
}

fn mount_bundled(cx: &mut TestAppContext, command: &str, arguments: &[(&str, &str)]) -> Mounted {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_shell::init(cx);
    });
    let runtime =
        cx.update(|cx| ShellRuntime::new_with_components(cx, components().unwrap()).unwrap());
    let window = cx.add_window(|_, _| Empty);
    let mut cx = VisualTestContext::from_window(*window.deref(), cx);
    let effects = Rc::new(RefCell::new(Vec::new()));
    let root = std::env::temp_dir().join(format!(
        "launcher-bundled-{}-{}",
        command.replace('/', "-"),
        std::process::id()
    ));
    std::fs::remove_dir_all(&root).ok();
    let view = open_bundled(&runtime, &mut cx, &effects, &root, command, arguments);
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

impl Mounted {
    /// Opens another bundled command on the same runtime and data directory.
    fn open_bundled(&mut self, command: &str, arguments: &[(&str, &str)]) {
        self.effects.borrow_mut().clear();
        self.view = open_bundled(
            &self.runtime,
            &mut self.cx,
            &self.effects,
            &self.root,
            command,
            arguments,
        );
        self.settle();
    }
}

fn action_titles(item: &Item) -> Vec<String> {
    item.actions()
        .all_actions()
        .map(|action| action.title().to_string())
        .collect()
}

#[gpui::test]
fn test_bundled_gpui_kit_commands(cx: &mut TestAppContext) {
    let mut mounted = mount_bundled(cx, "com.gpui-kit.links/links", &[]);
    let list = mounted.list();
    assert_eq!(list.sections().len(), 2);
    let home = list.items().next().unwrap();
    assert_eq!(
        action_titles(home),
        [
            "Open in Browser",
            "Copy URL",
            "Copy as Markdown",
            "Show Details"
        ]
    );
    assert!(matches!(
        home.primary_action().unwrap().effect(),
        Effect::OpenUrl(_)
    ));

    mounted.open_bundled("com.gpui-kit.links/checklist", &[]);
    // The first task starts done; its primary action undoes that.
    let list = mounted.list();
    assert!(list.placeholder().unwrap().contains("remaining"));
    let first_action = |list: &ListModel| {
        list.items()
            .next()
            .unwrap()
            .primary_action()
            .unwrap()
            .title()
            .to_string()
    };
    assert_eq!(first_action(&list), "Mark as Not Done");
    let Effect::Run(toggle) = list
        .items()
        .next()
        .unwrap()
        .primary_action()
        .unwrap()
        .effect()
        .clone()
    else {
        panic!("the primary action toggles the task");
    };
    mounted.call(|window, cx| toggle.run(window, cx));
    assert_eq!(first_action(&mounted.list()), "Mark as Done");

    mounted.open_bundled("com.gpui-kit.links/copy-date", &[]);
    let effects = mounted.effects();
    assert!(matches!(&effects[0], Effect::Copy(date) if date.len() == "2026-01-01".len()));
    assert!(matches!(&effects[1], Effect::ShowHud(text) if text.starts_with("Copied ")));

    mounted.open_bundled("com.gpui-kit.links/search-docs", &[("query", "dock")]);
    let effects = mounted.effects();
    assert!(matches!(
        &effects[0],
        Effect::OpenUrl(url) if url.contains("duckduckgo.com") && url.contains("dock")
    ));
    assert!(matches!(&effects[1], Effect::CloseWindow));
}

/// Only what happens before a request is sent: the test never goes online.
#[gpui::test]
fn test_bundled_repository_search_command(cx: &mut TestAppContext) {
    let mut mounted = mount_bundled(cx, "com.gpui-kit.github/search-repositories", &[]);
    let list = mounted.list();
    assert!(!list.is_filtering(), "the command runs the search itself");
    assert!(!list.is_loading());
    assert_eq!(list.empty_title().unwrap().as_ref(), "Search GitHub");

    // Typing waits for a pause before searching; clearing the query cancels it.
    let typed = list.on_query_change().cloned().unwrap();
    mounted.call(|window, cx| typed.call("gpui".into(), window, cx));
    let list = mounted.list();
    assert!(list.is_loading());
    assert_eq!(list.empty_title().unwrap().as_ref(), "Searching…");
    mounted.call(|window, cx| typed.call("".into(), window, cx));
    let list = mounted.list();
    assert!(!list.is_loading());
    assert_eq!(list.empty_title().unwrap().as_ref(), "Search GitHub");
}

/// Every command of every extension in the store loads, and renders its
/// first page (or menu) without the network: an extension must show
/// something useful, or say what is wrong, when a request fails.
#[gpui::test]
fn test_store_extensions_render_offline(cx: &mut TestAppContext) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("store/extensions");
    let catalog = Catalog::discover(std::slice::from_ref(&root));
    let commands: Vec<(String, crate::extensions::CommandMode, Vec<String>)> = catalog
        .commands()
        .map(|(_, command)| {
            (
                command.id().to_string(),
                command.mode(),
                command
                    .arguments()
                    .iter()
                    .filter(|argument| argument.required)
                    .map(|argument| argument.name.clone())
                    .collect(),
            )
        })
        .collect();
    assert!(!commands.is_empty(), "the store has extensions");
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_shell::init(cx);
    });
    let runtime =
        cx.update(|cx| ShellRuntime::new_with_components(cx, components().unwrap()).unwrap());
    let window = cx.add_window(|_, _| Empty);
    let mut cx = VisualTestContext::from_window(*window.deref(), cx);
    let effects = Rc::new(RefCell::new(Vec::new()));
    let data = std::env::temp_dir().join(format!("launcher-store-{}", std::process::id()));
    std::fs::remove_dir_all(&data).ok();
    let arguments_of = |required: &[String]| -> Vec<(String, String)> {
        required
            .iter()
            .map(|name| (name.clone(), "test".to_owned()))
            .collect()
    };
    let (first, _, first_required) = &commands[0];
    let first_arguments = arguments_of(first_required);
    let first_arguments: Vec<(&str, &str)> = first_arguments
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let view = open_in(
        &root,
        true,
        &runtime,
        &mut cx,
        &effects,
        &data,
        first,
        &first_arguments,
    );
    let mut mounted = Mounted {
        runtime: runtime.clone(),
        view,
        cx,
        effects: effects.clone(),
        root: data.clone(),
    };
    let mut failures = Vec::new();
    for (ix, (command, mode, required)) in commands.iter().enumerate() {
        if ix > 0 {
            let arguments = arguments_of(required);
            let arguments: Vec<(&str, &str)> = arguments
                .iter()
                .map(|(n, v)| (n.as_str(), v.as_str()))
                .collect();
            mounted.view = open_in(
                &root,
                true,
                &runtime,
                &mut mounted.cx,
                &effects,
                &data,
                command,
                &arguments,
            );
        }
        mounted.settle();
        let result = match mode {
            crate::extensions::CommandMode::View => match mounted.model() {
                Ok(PageModel::Failure { title, message }) => Err(format!("{title}: {message}")),
                Ok(_) => Ok(()),
                Err(error) => Err(error),
            },
            crate::extensions::CommandMode::MenuBar => mounted.menu_bar().map(|_| ()),
            crate::extensions::CommandMode::NoView => {
                let view = mounted.view.clone();
                mounted
                    .cx
                    .update(|_, cx| view.read(cx).build_error().map(str::to_owned))
                    .map_or(Ok(()), Err)
            }
        };
        if let Err(error) = result {
            failures.push(format!("{command}: {error}"));
        }
    }
    std::fs::remove_dir_all(&data).ok();
    assert!(failures.is_empty(), "{failures:#?}");
}
