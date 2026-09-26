//! The host embedding surface: applications loaded under an explicit policy,
//! and views a host builds out of a script callback.
//!
//! Both are what a host running several extensions on one runtime needs — a
//! launcher that tells which extension called `launch()` by the module
//! instance that answered, and that pushes a page an extension's action built.

use std::{
    cell::{Cell, RefCell},
    ops::Deref,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use gpui::{
    AnyElement, Entity, IntoElement as _, Render as _, TestAppContext, VisualTestContext, div,
};

use crate::{
    ArgumentDescriptor, ArgumentSchema, COMPONENT_REGISTRY_API_VERSION, Capabilities,
    ComponentArgument, ComponentCallback, ComponentDescriptor, ComponentMaterializer,
    ComponentPayload, ComponentRegistry, ConstructorDescriptor, HostModule, HostValue,
    MaterializeRequest, ScriptView, ShellRuntime, policy::Policy,
};

static NEXT_APP: AtomicU64 = AtomicU64::new(0);

struct TempApp(PathBuf);

impl TempApp {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gpui-shell-embedding-{}-{}",
            std::process::id(),
            NEXT_APP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create application directory");
        std::fs::write(path.join("main.js"), source).expect("write application entry");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A `launcher` module that answers with the name it was built for and logs
/// each call, so a test can tell which instance a script reached.
fn launcher_module(name: &'static str, calls: Rc<RefCell<Vec<String>>>) -> HostModule {
    HostModule::new("launcher").function("whoami", move |_| {
        calls.borrow_mut().push(name.to_owned());
        Ok(HostValue::from(name))
    })
}

fn launcher_policy(name: &'static str, root: &Path) -> (Rc<Policy>, Rc<RefCell<Vec<String>>>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let policy = Policy::new()
        .with_application(name)
        .with_capabilities(Capabilities::new().read_roots([root.to_path_buf()]))
        .with_host_module(launcher_module(name, calls.clone()))
        .expect("`launcher` is not a reserved name");
    (Rc::new(policy), calls)
}

const COMMAND: &str = r#"
import { View } from "gpui-kit";
import { v_flex } from "gpui-base";
import { whoami } from "launcher";

const loadedBy = whoami();

export default class Command extends View {
  init() { this.initializedBy = whoami(); }
  render() {
    return v_flex().child(`load:${loadedBy}|init:${this.initializedBy}|render:${whoami()}`);
  }
}
"#;

#[gpui::test]
fn applications_load_and_mount_under_their_own_policy(cx: &mut TestAppContext) {
    cx.update(crate::init);
    // A module of the same name in the default policy must not answer for an
    // application loaded under a policy of its own.
    let global_calls = Rc::new(RefCell::new(Vec::new()));
    crate::export_module(launcher_module("global", global_calls.clone()))
        .expect("`launcher` is not a reserved name");

    let first_app = TempApp::new(COMMAND);
    let second_app = TempApp::new(COMMAND);
    let (first_policy, first_calls) = launcher_policy("first", first_app.path());
    let (second_policy, second_calls) = launcher_policy("second", second_app.path());

    let runtime = ShellRuntime::new_isolated().expect("runtime");
    let window = cx.add_window(|_, _| Empty);
    let mut context = VisualTestContext::from_window(*window.deref(), cx);

    let (first, second) = context.update(|window, cx| {
        let first = runtime
            .load_application_with_policy(
                first_app.path(),
                "main.js",
                first_policy.clone(),
                window,
                cx,
            )
            .expect("load the first application");
        let second = runtime
            .load_application_with_policy(
                second_app.path(),
                "main.js",
                second_policy.clone(),
                window,
                cx,
            )
            .expect("load the second application");
        (
            runtime
                .mount_application(&first, window, cx)
                .expect("mount the first application"),
            runtime
                .mount_application(&second, window, cx)
                .expect("mount the second application"),
        )
    });

    context.update(|_, cx| {
        assert!(Rc::ptr_eq(&first.read(cx).policy(), &first_policy));
        assert!(Rc::ptr_eq(&second.read(cx).policy(), &second_policy));
        assert_eq!(first.read(cx).policy().application(), "first");
        assert!(first.read(cx).policy().capabilities().has_read_access());
    });

    draw(&mut context, &first);
    draw(&mut context, &second);
    assert!(
        snapshot_text(&mut context, &first).contains("load:first|init:first|render:first"),
        "{}",
        snapshot_text(&mut context, &first)
    );
    assert!(
        snapshot_text(&mut context, &second).contains("load:second|init:second|render:second"),
        "{}",
        snapshot_text(&mut context, &second)
    );
    assert_eq!(first_calls.borrow().as_slice(), ["first"; 3]);
    assert_eq!(second_calls.borrow().as_slice(), ["second"; 3]);
    assert!(global_calls.borrow().is_empty());

    // The default path still links against the default policy's modules.
    let plain_app = TempApp::new(COMMAND);
    let plain = runtime
        .load_application(plain_app.path(), "main.js")
        .expect("load under the default policy");
    let plain = context
        .update(|window, cx| runtime.mount_application(&plain, window, cx))
        .expect("mount under the default policy");
    draw(&mut context, &plain);
    assert!(snapshot_text(&mut context, &plain).contains("load:global|init:global|render:global"));

    // A policy that granted no `launcher` does not fall back to the default's.
    let bare_app = TempApp::new(COMMAND);
    let error = context
        .update(|window, cx| {
            runtime.load_application_with_policy(
                bare_app.path(),
                "main.js",
                Rc::new(Policy::new()),
                window,
                cx,
            )
        })
        .err()
        .expect("a policy without the module cannot link the import");
    assert!(error.to_string().contains("launcher"), "{error:#}");

    crate::clear_exported_modules();
}

thread_local! {
    static CALLBACKS: RefCell<Vec<ComponentCallback>> = const { RefCell::new(Vec::new()) };
}

/// Captures its callback argument the way a real adapter would, so the test
/// holds exactly what a host holds.
struct CapturingMaterializer;

impl ComponentMaterializer for CapturingMaterializer {
    fn materialize(&self, request: MaterializeRequest<'_>) -> anyhow::Result<AnyElement> {
        let arguments = request
            .payload()
            .downcast_ref::<Vec<ComponentArgument>>()
            .expect("the constructor stores its arguments")
            .clone();
        let callback = request.resolve_callback(&arguments[0])?;
        CALLBACKS.with(|callbacks| callbacks.borrow_mut().push(callback));
        Ok(div().into_any_element())
    }
}

fn capturing_runtime() -> Rc<ShellRuntime> {
    let mut registry = ComponentRegistry::new(
        COMPONENT_REGISTRY_API_VERSION,
        crate::DEFAULT_COMPONENT_MODULE,
    )
    .expect("registry");
    registry
        .register(
            ComponentDescriptor::new("Action", Arc::new(CapturingMaterializer))
                .with_constructors(vec![ConstructorDescriptor::new(
                    "Action",
                    vec![ArgumentDescriptor::new(
                        "handler",
                        ArgumentSchema::Callback("(cx: Context) => unknown"),
                    )],
                    |arguments| Ok(ComponentPayload::new(arguments.to_vec())),
                )])
                .with_methods(Vec::new()),
        )
        .expect("register Action");
    ShellRuntime::new_isolated_with_components(registry.freeze().expect("freeze")).expect("runtime")
}

fn take_callbacks() -> Vec<ComponentCallback> {
    CALLBACKS.with(|callbacks| std::mem::take(&mut *callbacks.borrow_mut()))
}

const PUSHING: &str = r#"
import { View, div } from "gpui-kit";
import { Action } from "gpui-component";
import { whoami } from "launcher";

class Detail extends View {
  init(props, cx) {
    this.repo = props.repo;
    this.count = 0;
    this.state = "pending";
    this.focus = cx.focus_handle();
    this.ticker = cx.timer.every(1000, () => {});
    cx.spawn(async (cx) => {
      await cx.sleep(10);
      this.state = "loaded";
      cx.notify();
    });
  }
  render() {
    return div()
      .child(`detail:${this.repo}|count:${this.count}|${this.state}|${whoami()}`)
      .child(new Action((cx) => { this.count += 1; cx.notify(); }));
  }
}

class Broken extends View {
  init(_props, cx) {
    this.focus = cx.focus_handle();
    throw new Error("init exploded");
  }
  render() { return div(); }
}

export default class List extends View {
  init() {
    this.existing = new Detail({ repo: "existing" });
  }
  render() {
    return div()
      .child(new Action(() => new Detail({ repo: "zed" })))
      .child(new Action(() => 42))
      .child(new Action(() => { throw new Error("no page for you"); }))
      .child(new Action(() => this.existing))
      .child(new Action(() => new Broken({})));
  }
}
"#;

#[gpui::test]
fn a_callback_returning_a_view_becomes_a_hosted_entity(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let app = TempApp::new(PUSHING);
    let (policy, calls) = launcher_policy("extension", app.path());
    let runtime = capturing_runtime();
    let window = cx.add_window(|_, _| Empty);
    let mut context = VisualTestContext::from_window(*window.deref(), cx);

    let list = context
        .update(|window, cx| {
            let loaded = runtime.load_application_with_policy(
                app.path(),
                "main.js",
                policy.clone(),
                window,
                cx,
            )?;
            runtime.mount_application(&loaded, window, cx)
        })
        .expect("mount the list");
    take_callbacks();
    draw(&mut context, &list);
    let actions = take_callbacks();
    assert_eq!(actions.len(), 5, "one callback per Action");

    context.run_until_parked();
    let baseline_records = runtime.entities().len();
    let baseline_tasks = crate::engine::quickjs::task_count();

    let (first, second) = context.update(|window, cx| {
        (
            actions[0]
                .invoke_view(window, cx)
                .expect("push a detail page"),
            actions[0].invoke_view(window, cx).expect("push another"),
        )
    });
    assert!(
        crate::engine::quickjs::task_count() >= baseline_tasks + 2,
        "each pushed view's init left a repeating timer"
    );
    context.update(|_, cx| {
        assert!(Rc::ptr_eq(&first.read(cx).policy(), &policy));
    });
    assert!(
        runtime.entities().len() > baseline_records,
        "init retained a focus handle under the pushed view"
    );

    // The host renders the pushed view itself rather than mounting it.
    render_directly(&mut context, &first);
    let text = snapshot_text(&mut context, &first);
    assert!(
        text.contains("detail:zed|count:0|pending|extension"),
        "{text}"
    );
    let detail_actions = take_callbacks();
    assert_eq!(detail_actions.len(), 1);
    render_directly(&mut context, &second);
    take_callbacks();

    // Work `init` started belongs to the pushed entity: its notify reaches
    // an observer of that entity.
    let notified = Rc::new(Cell::new(0));
    let _subscription = context.update(|_, cx| {
        let notified = notified.clone();
        cx.observe(&first, move |_, _| notified.set(notified.get() + 1))
    });
    context.executor().advance_clock(Duration::from_millis(10));
    context.run_until_parked();
    assert!(notified.get() > 0, "the init task did not notify its view");
    render_directly(&mut context, &first);
    assert!(snapshot_text(&mut context, &first).contains("loaded"));
    let detail_actions = take_callbacks();

    // An event from the pushed view changes that view's state only.
    let before = notified.get();
    context
        .update(|window, cx| detail_actions[0].invoke(window, cx))
        .expect("the pushed view's own callback runs");
    assert!(notified.get() > before, "the event did not notify its view");
    render_directly(&mut context, &first);
    render_directly(&mut context, &second);
    take_callbacks();
    assert!(snapshot_text(&mut context, &first).contains("count:1"));
    assert!(snapshot_text(&mut context, &second).contains("count:0"));
    assert!(calls.borrow().iter().all(|caller| caller == "extension"));

    // Dropping the entities is the release: their tasks and retained records
    // go with them.
    // Only the two repeating timers are left of the pushed views' work.
    let tasks_before_drop = crate::engine::quickjs::task_count();
    let weak = (first.downgrade(), second.downgrade());
    drop((first, second));
    context.update(|_, _| {});
    context.run_until_parked();
    assert!(
        weak.0.upgrade().is_none(),
        "something still holds the first view"
    );
    assert!(
        weak.1.upgrade().is_none(),
        "something still holds the second view"
    );
    assert_eq!(runtime.entities().len(), baseline_records);
    let tasks_after_drop = crate::engine::quickjs::task_count();
    assert_eq!(tasks_after_drop, tasks_before_drop - 2);

    // Failures are errors, and leave nothing retained behind.
    let errors = context.update(|window, cx| {
        actions[1..]
            .iter()
            .map(|action| {
                action
                    .invoke_view(window, cx)
                    .err()
                    .expect("this callback cannot produce a view")
                    .to_string()
            })
            .collect::<Vec<_>>()
    });
    assert!(
        errors[0].contains("must return a new View instance"),
        "{}",
        errors[0]
    );
    assert!(errors[0].contains("number"), "{}", errors[0]);
    assert!(errors[1].contains("no page for you"), "{}", errors[1]);
    assert!(errors[2].contains("already existed"), "{}", errors[2]);
    assert!(errors[3].contains("init exploded"), "{}", errors[3]);
    context.run_until_parked();
    assert_eq!(runtime.entities().len(), baseline_records);
    assert_eq!(crate::engine::quickjs::task_count(), tasks_after_drop);
}

const UTILS: &str = r#"
import { View } from "gpui-kit";
import { show_toast } from "launcher/api";

let instances = 0;

export class Counter {
  constructor() {
    this.value = 0;
    this.instance = ++instances;
  }
  bump() {
    this.value += 1;
    show_toast(`bumped:${this.value}`);
    return this.value;
  }
}

export const isView = (value) => value instanceof View;

export const tryEval = () => {
  try {
    eval("1");
    return "eval-allowed";
  } catch (_) {
    return "eval-withheld";
  }
};
"#;

const USES_UTILS: &str = r#"
import { View } from "gpui-kit";
import { v_flex } from "gpui-base";
import { Counter, isView, tryEval } from "launcher/utils";

export default class Command extends View {
  init() {
    this.counter = new Counter();
    this.counter.bump();
  }
  render() {
    return v_flex().child(
      `instance:${this.counter.instance}|value:${this.counter.value}|view:${isView(this)}|${tryEval()}`,
    );
  }
}
"#;

fn utils_policy(name: &'static str) -> (Rc<Policy>, Rc<RefCell<Vec<String>>>) {
    let toasts = Rc::new(RefCell::new(Vec::new()));
    let log = toasts.clone();
    let policy = Policy::new()
        .with_application(name)
        .with_host_module(HostModule::new("launcher/api").function(
            "show_toast",
            move |arguments| {
                log.borrow_mut().push(arguments.string(0)?.to_owned());
                Ok(HostValue::Null)
            },
        ))
        .and_then(|policy| policy.with_host_module(HostModule::source("launcher/utils", UTILS)))
        .expect("neither name is reserved");
    (Rc::new(policy), toasts)
}

#[gpui::test]
fn a_source_host_module_is_imported_under_the_importing_policy(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (first_policy, first_toasts) = utils_policy("first");
    let (second_policy, second_toasts) = utils_policy("second");
    let runtime = ShellRuntime::new_isolated().expect("runtime");
    let window = cx.add_window(|_, _| Empty);
    let mut context = VisualTestContext::from_window(*window.deref(), cx);

    let apps = [
        TempApp::new(USES_UTILS),
        TempApp::new(USES_UTILS),
        TempApp::new(USES_UTILS),
    ];
    let views = context.update(|window, cx| {
        [&first_policy, &first_policy, &second_policy]
            .into_iter()
            .zip(&apps)
            .map(|(policy, app)| {
                let loaded = runtime
                    .load_application_with_policy(app.path(), "main.js", policy.clone(), window, cx)
                    .expect("load an application importing the source module");
                runtime
                    .mount_application(&loaded, window, cx)
                    .expect("mount it")
            })
            .collect::<Vec<_>>()
    });

    let texts = views
        .iter()
        .map(|view| {
            draw(&mut context, view);
            snapshot_text(&mut context, view)
        })
        .collect::<Vec<_>>();
    // One module instance per policy's set of modules: the two applications
    // under the first policy share it, the second policy has its own.
    assert!(
        texts[0].contains("instance:1|value:1|view:true"),
        "{}",
        texts[0]
    );
    assert!(
        texts[1].contains("instance:2|value:1|view:true"),
        "{}",
        texts[1]
    );
    assert!(
        texts[2].contains("instance:1|value:1|view:true"),
        "{}",
        texts[2]
    );
    // The source runs inside the application's sandbox, with nothing extra.
    assert!(texts.iter().all(|text| text.contains("eval-withheld")));
    // Its own import reached the Rust module of the policy it was linked under.
    assert_eq!(first_toasts.borrow().as_slice(), ["bumped:1", "bumped:1"]);
    assert_eq!(second_toasts.borrow().as_slice(), ["bumped:1"]);
}

#[gpui::test]
fn a_source_host_module_is_refused_where_a_host_module_would_be(cx: &mut TestAppContext) {
    cx.update(crate::init);
    for reserved in ["gpui-kit", "path"] {
        let error = Policy::new()
            .with_host_module(HostModule::source(reserved, "export const x = 1;"))
            .err()
            .expect("a reserved name is refused");
        assert!(error.message().contains("reserved"), "{error}");
        let error = crate::export_module(HostModule::source(reserved, "export const x = 1;"))
            .expect_err("a reserved name is refused globally too");
        assert!(error.message().contains("reserved"), "{error}");
    }

    let error = Policy::new()
        .with_host_module(
            HostModule::source("mixed", "export const x = 1;")
                .function("y", |_| Ok(HostValue::Null)),
        )
        .err()
        .expect("a source module exports only its source");
    assert!(error.message().contains("source module"), "{error}");

    // A source module has no directory for a relative import to start from.
    let policy = Policy::new()
        .with_host_module(HostModule::source(
            "relative",
            "import { x } from \"./sibling.js\";\nexport const y = x;",
        ))
        .expect("the name is not reserved");
    let app = TempApp::new(
        r#"
import { View } from "gpui-kit";
import { y } from "relative";
export default class Probe extends View { render() { return `${y}`; } }
"#,
    );
    std::fs::write(app.path().join("sibling.js"), "export const x = 1;").expect("write sibling");
    let runtime = ShellRuntime::new_isolated().expect("runtime");
    let window = cx.add_window(|_, _| Empty);
    let mut context = VisualTestContext::from_window(*window.deref(), cx);
    let error = context
        .update(|window, cx| {
            runtime.load_application_with_policy(app.path(), "main.js", Rc::new(policy), window, cx)
        })
        .err()
        .expect("a relative import from a source module is refused");
    assert!(
        error.to_string().contains("only bare specifiers"),
        "{error:#}"
    );
}

/// What a host that takes a data model out of a view does: call `render`
/// inside `entity.update` and keep the element to itself.
fn render_directly(context: &mut VisualTestContext, view: &Entity<ScriptView>) {
    context.update(|window, cx| {
        view.update(cx, |view, cx| {
            drop(view.render(window, cx).into_any_element());
        })
    });
}

fn draw(context: &mut VisualTestContext, view: &Entity<ScriptView>) {
    let view = view.clone();
    context.draw(
        gpui::Point::default(),
        gpui::size(gpui::px(400.), gpui::px(300.)),
        move |_, _| view.into_any_element(),
    );
}

fn snapshot_text(context: &mut VisualTestContext, view: &Entity<ScriptView>) -> String {
    context.update(|_, cx| {
        view.read(cx)
            .snapshot()
            .map(crate::RenderSnapshot::debug_tree)
            .unwrap_or_else(|| {
                view.read(cx)
                    .build_error()
                    .unwrap_or("no snapshot")
                    .to_owned()
            })
    })
}

struct Empty;

impl gpui::Render for Empty {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
    }
}
