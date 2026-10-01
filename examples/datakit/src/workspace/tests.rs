//! The main window driven as a person would: keys, menus and the platform's
//! file dialogs, against SQLite data sources in a data directory of the
//! test's own.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use datakit_driver::ConnectionProfile;
use datakit_store::DataSourceFile;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::{
    AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions, px, size,
};

use super::*;
use crate::{console::SessionState, history::HistoryLog};

/// A directory that is removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("datakit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Start DataKit as `main` does, keeping its files in `data`, and open the
/// main window.
fn start(data: &Path, cx: &mut TestAppContext) -> (Entity<Workspace>, VisualTestContext) {
    cx.executor().allow_parking();
    let data = data.to_path_buf();
    let window = cx.update(|cx| {
        gpui_kit::init(cx);
        rust_i18n::set_locale("en");
        Services::init_for_test(data, cx).unwrap();
        Settings::init("en", cx);
        Navigation::init(cx);
        DataSources::init(cx);
        HistoryLog::init(cx);
        crate::recent::Recent::init(cx);
        crate::results::init(cx);
        Sessions::init(cx);
        crate::console::init(cx);
        crate::explorer::init(cx);
        crate::files::init(cx);
        crate::table_editor::init(cx);
        super::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.), px(820.)),
                cx,
            ))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .unwrap()
    });
    let (handle, workspace) = window;
    let cx = VisualTestContext::from_window(handle.into(), cx);
    cx.run_until_parked();
    (workspace, cx)
}

/// Run the app until `done`; database work happens on the IO runtime's
/// threads, outside the test executor.
fn wait_until(cx: &mut VisualTestContext, what: &str, done: impl Fn(&mut App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if cx.update(|_, cx| done(cx)) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Let the consoles' half-second save debounce pass.
fn flush_saves(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
}

fn consoles(cx: &mut VisualTestContext) -> Vec<Entity<ConsolePanel>> {
    cx.update(|_, cx| Sessions::global(cx).read(cx).consoles())
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[gpui_kit::test]
fn settings_and_sql_files(cx: &mut TestAppContext) {
    let scratch = Scratch::new("gui-files");
    let data = scratch.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("settings.json"),
        r#"{ "appearance": "dark", "uppercase_keywords": true }"#,
    )
    .unwrap();
    let scripts = scratch.join("scripts");
    std::fs::create_dir_all(scripts.join("reports")).unwrap();
    std::fs::write(scripts.join("a.sql"), "select 1;").unwrap();
    std::fs::write(scripts.join("reports").join("b.sql"), "select 2;").unwrap();

    let (_workspace, mut cx) = start(&data, cx);

    // The saved appearance is applied to the window.
    cx.update(|_, cx| assert!(cx.theme().is_dark(), "the window is dark"));

    // Settings open from their key, and close with Escape.
    cx.simulate_keystrokes("secondary-,");
    cx.run_until_parked();
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));

    // Ctrl-O asks for files and opens each in a console.
    cx.simulate_keystrokes("secondary-o");
    cx.run_until_parked();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && options.multiple);
        Some(vec![scripts.join("a.sql")])
    });
    cx.run_until_parked();
    let opened = consoles(&mut cx);
    assert_eq!(opened.len(), 1);
    let console = opened[0].clone();
    cx.update(|_, cx| {
        assert_eq!(console.read(cx).file(), Some(&scripts.join("a.sql")));
        assert_eq!(console.read(cx).name().as_ref(), "a.sql");
    });

    // Typing saves back to the file.
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-down"
    } else {
        "ctrl-end"
    });
    cx.simulate_input("\nselect 3;");
    flush_saves(&mut cx);
    assert_eq!(read(&scripts.join("a.sql")), "select 1;\nselect 3;");

    // Opening the file again shows the same console.
    cx.simulate_keystrokes("secondary-o");
    cx.run_until_parked();
    cx.simulate_path_prompt_response(|_| Some(vec![scripts.join("a.sql")]));
    cx.run_until_parked();
    assert_eq!(consoles(&mut cx).len(), 1);

    // Save As moves the console to the new file.
    cx.simulate_keystrokes("secondary-shift-s");
    cx.run_until_parked();
    let copy = scripts.join("copy.sql");
    cx.simulate_new_path_selection(|_| Some(copy.clone()));
    cx.run_until_parked();
    assert_eq!(read(&copy), "select 1;\nselect 3;");
    cx.update(|_, cx| {
        assert_eq!(console.read(cx).file(), Some(&copy));
        assert_eq!(console.read(cx).name().as_ref(), "copy.sql");
    });
    cx.simulate_input("\nselect 4;");
    flush_saves(&mut cx);
    assert_eq!(read(&copy), "select 1;\nselect 3;\nselect 4;");
    assert_eq!(read(&scripts.join("a.sql")), "select 1;\nselect 3;");

    // Attaching a folder remembers it.
    cx.dispatch_action(AttachFolder);
    cx.run_until_parked();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files);
        Some(vec![scripts.clone()])
    });
    cx.run_until_parked();
    let folders: Vec<PathBuf> =
        serde_json::from_str(&read(&data.join("folders.json"))).unwrap_or_default();
    assert_eq!(folders, std::slice::from_ref(&scripts));

    // Ctrl-E lists a.sql, which no console edits since Save As; Enter
    // opens it again.
    cx.simulate_keystrokes("secondary-e");
    cx.run_until_parked();
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.simulate_input("a.sql");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let opened = consoles(&mut cx);
    assert_eq!(opened.len(), 2);
    assert!(cx.update(|_, cx| {
        opened
            .iter()
            .any(|console| console.read(cx).file() == Some(&scripts.join("a.sql")))
    }));
}

#[gpui_kit::test]
fn switching_data_source_and_completing_a_join(cx: &mut TestAppContext) {
    let scratch = Scratch::new("gui-join");
    let data = scratch.join("data");
    let first = scratch.join("first.sqlite3");
    let second = scratch.join("second.sqlite3");
    DataSourceFile::new(data.join("data-sources.json"))
        .save(&[
            ConnectionProfile::new("sqlite", 0)
                .with_name("First")
                .with_option(ConnectionProfile::FILE, first.display().to_string()),
            ConnectionProfile::new("sqlite", 0)
                .with_name("Second")
                .with_option(ConnectionProfile::FILE, second.display().to_string()),
        ])
        .unwrap();
    let script = scratch.join("join.sql");
    std::fs::write(&script, "").unwrap();

    let (workspace, mut cx) = start(&data, cx);

    // A file opened with no data source selected runs nowhere until one
    // is chosen.
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_file(script.clone(), window, cx)
        })
    });
    cx.run_until_parked();
    let console = consoles(&mut cx).remove(0);
    assert!(cx.update(|_, cx| console.read(cx).data_source().is_none()));

    let second_source = cx.update(|_, cx| DataSources::global(cx).read(cx).items()[1].clone());
    cx.update(|window, cx| {
        console.update(cx, |console, cx| {
            console.set_data_source(second_source.clone(), window, cx)
        })
    });
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| console.read(cx).data_source().map(|s| s.read(cx).name())),
        Some("Second".into())
    );

    cx.update(|window, cx| {
        console.update(cx, |console, cx| {
            console.run_sql(
                "create table customers (id integer primary key, name text);\n\
                 create table orders (id integer primary key, \
                 customer_id integer references customers (id), total real);",
                window,
                cx,
            )
        })
    });
    wait_until(&mut cx, "the tables to be created", |cx| {
        console.read(cx).session_state() == SessionState::Idle
    });
    assert!(second.exists(), "the statements ran against Second");
    assert!(!first.exists(), "nothing ran against First");

    // Type a join and accept the first completion after `on`.
    cx.update(|window, cx| {
        let focus = console.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.simulate_keystrokes("secondary-a backspace");
    cx.simulate_input("select * from orders o join customers c on o");
    // The first keystrokes load the catalog; ask again once it is there.
    wait_until(&mut cx, "the catalog", |cx| {
        second_source
            .read(cx)
            .loaded_schema("main")
            .and_then(|schema| schema.relations())
            .is_some_and(|relations| relations.len() == 2)
    });
    cx.simulate_keystrokes("backspace");
    cx.simulate_input("o");
    cx.run_until_parked();
    // Enter takes the completion and nothing else.
    cx.simulate_keystrokes("enter");
    flush_saves(&mut cx);
    assert_eq!(
        read(&script),
        "select * from orders o join customers c on o.customer_id = c.id"
    );

    // Once the menu is dismissed, Ctrl-Space asks again without typing.
    cx.simulate_keystrokes("secondary-a backspace");
    cx.simulate_input("select * from ");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    let editor = cx.update(|_, cx| console.read(cx).editor().clone());
    assert!(cx.update(|_, cx| !editor.read(cx).completion_menu_state().open));
    cx.simulate_keystrokes("ctrl-space");
    cx.run_until_parked();
    cx.update(|_, cx| {
        let menu = editor.read(cx).completion_menu_state();
        assert!(menu.open, "Ctrl-Space opens the completions");
        assert!(menu.items.iter().any(|item| item.label == "orders"));
    });
}

/// One SQLite data source named Local, and a console on it.
fn local_console(
    scratch: &Scratch,
    cx: &mut TestAppContext,
) -> (Entity<Workspace>, VisualTestContext, Entity<ConsolePanel>) {
    let data = scratch.join("data");
    DataSourceFile::new(data.join("data-sources.json"))
        .save(&[ConnectionProfile::new("sqlite", 0)
            .with_name("Local")
            .with_option(
                ConnectionProfile::FILE,
                scratch.join("local.sqlite3").display().to_string(),
            )])
        .unwrap();
    let (workspace, mut cx) = start(&data, cx);
    let source = cx.update(|_, cx| DataSources::global(cx).read(cx).items()[0].clone());
    let script = scratch.join("local.sql");
    std::fs::write(&script, "").unwrap();
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| workspace.open_file(script, window, cx))
    });
    cx.run_until_parked();
    let console = consoles(&mut cx).remove(0);
    cx.update(|window, cx| {
        console.update(cx, |console, cx| {
            console.set_data_source(source, window, cx)
        })
    });
    cx.run_until_parked();
    (workspace, cx, console)
}

fn run(console: &Entity<ConsolePanel>, sql: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| console.update(cx, |console, cx| console.run_sql(sql, window, cx)));
    wait_until(cx, sql, |cx| {
        console.read(cx).session_state() == SessionState::Idle
    });
}

#[gpui_kit::test]
fn a_result_is_filtered_on_the_server(cx: &mut TestAppContext) {
    let scratch = Scratch::new("gui-filter");
    let (_workspace, mut cx, console) = local_console(&scratch, cx);
    run(
        &console,
        "create table t (n integer);\n\
         insert into t values (1), (2), (3), (4);\n\
         select n from t",
        &mut cx,
    );
    let result = cx.update(|_, cx| console.read(cx).result_views().remove(0));
    let numbers = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            result
                .read(cx)
                .displayed_rows(cx)
                .iter()
                .map(|row| row[0].display().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(numbers(&mut cx), ["1", "2", "3", "4"]);

    // Enter in the WHERE box reads the statement again with the condition.
    let condition = cx.update(|_, cx| result.read(cx).condition().clone());
    cx.update(|window, cx| condition.update(cx, |input, cx| input.focus(window, cx)));
    cx.simulate_input("n > 2");
    cx.simulate_keystrokes("enter");
    wait_until(&mut cx, "the filtered rows", |cx| {
        result.read(cx).displayed_rows(cx).len() == 2
    });
    assert_eq!(numbers(&mut cx), ["3", "4"]);
}

#[gpui_kit::test]
fn a_read_only_data_source_refuses_changes(cx: &mut TestAppContext) {
    let scratch = Scratch::new("gui-read-only");
    let (_workspace, mut cx, console) = local_console(&scratch, cx);
    run(&console, "create table t (n integer)", &mut cx);

    let source = cx.update(|_, cx| console.read(cx).data_source().unwrap().clone());
    cx.update(|_, cx| {
        let profile = source
            .read(cx)
            .profile()
            .clone()
            .with_option(crate::datasource::READ_ONLY, "true");
        DataSources::global(cx).update(cx, |sources, cx| sources.update(profile, None, None, cx));
    });
    cx.run_until_parked();
    assert!(cx.update(|_, cx| source.read(cx).is_read_only()));

    // Nothing runs when one statement writes, not even the reads before it.
    run(
        &console,
        "select n from t;\ninsert into t values (1)",
        &mut cx,
    );
    assert!(cx.update(|_, cx| console.read(cx).result_views().is_empty()));
    run(&console, "select count(*) from t", &mut cx);
    let result = cx.update(|_, cx| console.read(cx).result_views().remove(0));
    let count = cx.update(|_, cx| {
        result.read(cx).displayed_rows(cx)[0][0]
            .display()
            .map(|s| s.to_string())
    });
    assert_eq!(count.as_deref(), Some("0"));
}

#[gpui_kit::test]
fn a_file_changed_on_disk_shows_in_its_console(cx: &mut TestAppContext) {
    let scratch = Scratch::new("gui-reload");
    let (_workspace, mut cx, console) = local_console(&scratch, cx);
    let script = scratch.join("local.sql");
    let editor = cx.update(|_, cx| console.read(cx).editor().clone());

    std::fs::write(&script, "select 42;").unwrap();
    // The watch reports from a thread of its own, then settles on the
    // test clock.
    let deadline = Instant::now() + Duration::from_secs(20);
    while cx.update(|_, cx| editor.read(cx).value().to_string()) != "select 42;" {
        assert!(Instant::now() < deadline, "the console never reloaded");
        std::thread::sleep(Duration::from_millis(20));
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
    }

    // Typing goes on where the caret was, and saves back.
    cx.update(|window, cx| {
        let focus = console.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.simulate_input("select 43;\n");
    flush_saves(&mut cx);
    assert_eq!(read(&script), "select 43;\nselect 42;");
}

#[gpui_kit::test]
fn renaming_a_table_changes_the_consoles_naming_it(cx: &mut TestAppContext) {
    let scratch = Scratch::new("gui-rename");
    let (_workspace, mut cx, console) = local_console(&scratch, cx);
    run(&console, "create table orders (id integer)", &mut cx);
    let source = cx.update(|_, cx| console.read(cx).data_source().unwrap().clone());
    cx.update(|_, cx| {
        source.update(cx, |source, cx| {
            source.request(
                crate::datasource::CatalogRequest::Objects("main".into()),
                cx,
            )
        })
    });
    wait_until(&mut cx, "the catalog", |cx| {
        source
            .read(cx)
            .loaded_schema("main")
            .and_then(|schema| schema.relations())
            .is_some_and(|relations| relations.len() == 1)
    });
    run(
        &console,
        "select o.id from orders o; select 'orders'",
        &mut cx,
    );

    let orders = crate::objects::ObjectRef::new(
        source.clone(),
        crate::objects::ObjectPath::relation("main", "orders"),
    );
    cx.update(|window, cx| crate::rename::rename_object(&orders, window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("purchases");
    cx.simulate_keystrokes("enter");
    let editor = cx.update(|_, cx| console.read(cx).editor().clone());
    wait_until(&mut cx, "the console to follow the rename", |cx| {
        editor.read(cx).value().contains("purchases")
    });
    assert_eq!(
        cx.update(|_, cx| editor.read(cx).value().to_string()),
        "select o.id from purchases o; select 'orders'"
    );
    run(&console, "select count(*) from purchases", &mut cx);
    assert_eq!(cx.update(|_, cx| console.read(cx).result_views().len()), 1);
}

#[gpui_kit::test]
fn find_usages_goes_to_where_a_table_is_named(cx: &mut TestAppContext) {
    let scratch = Scratch::new("gui-usages");
    let (_workspace, mut cx, console) = local_console(&scratch, cx);
    run(&console, "create table orders (id integer)", &mut cx);
    let source = cx.update(|_, cx| console.read(cx).data_source().unwrap().clone());
    cx.update(|_, cx| {
        source.update(cx, |source, cx| {
            source.request(
                crate::datasource::CatalogRequest::Objects("main".into()),
                cx,
            )
        })
    });
    wait_until(&mut cx, "the catalog", |cx| {
        source
            .read(cx)
            .loaded_schema("main")
            .and_then(|schema| schema.relations())
            .is_some_and(|relations| relations.len() == 1)
    });
    let text = "select id from orders;\nselect 'orders' from orders o";
    run(&console, text, &mut cx);

    // Alt-F7 on the first `orders` lists both; Enter goes to the first.
    let editor = cx.update(|_, cx| console.read(cx).editor().clone());
    cx.update(|window, cx| console.update(cx, |console, cx| console.select(16..16, window, cx)));
    cx.simulate_keystrokes("alt-f7");
    cx.run_until_parked();
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    let second = text.rfind("orders").unwrap();
    assert_eq!(
        cx.update(|_, cx| editor.read(cx).selected_range()),
        second..second + "orders".len()
    );

    // Ctrl-P inside a call shows what the routine takes.
    run(&console, "select coalesce(null, 2)", &mut cx);
    cx.update(|window, cx| console.update(cx, |console, cx| console.select(22..22, window, cx)));
    cx.simulate_keystrokes("secondary-p");
    cx.run_until_parked();
    let hover = cx.update(|_, cx| {
        editor
            .read(cx)
            .hover_popover()
            .map(|hover| hover.symbol_range.clone())
    });
    assert_eq!(hover, Some(7..15));
}

#[gpui_kit::test]
fn local_history_brings_back_text_that_ran(cx: &mut TestAppContext) {
    let scratch = Scratch::new("gui-local-history");
    let (_workspace, mut cx, console) = local_console(&scratch, cx);
    let editor = cx.update(|_, cx| console.read(cx).editor().clone());
    cx.update(|window, cx| {
        let focus = console.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.simulate_input("select 1");
    cx.dispatch_action(crate::console::ExecuteStatement);
    wait_until(&mut cx, "the statement", |cx| {
        console.read(cx).session_state() == SessionState::Idle
    });
    cx.simulate_keystrokes("secondary-a backspace");
    cx.simulate_input("select 2");
    flush_saves(&mut cx);

    // The latest version is the one that ran; reverting brings it back.
    cx.dispatch_action(crate::console::ShowLocalHistory);
    cx.run_until_parked();
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| editor.read(cx).value().to_string()),
        "select 1"
    );
}

#[gpui_kit::test]
fn comparing_data_writes_the_statements_that_sync_the_target(cx: &mut TestAppContext) {
    let scratch = Scratch::new("gui-data-compare");
    let data = scratch.join("data");
    let profile = |name: &str, file: &str| {
        ConnectionProfile::new("sqlite", 0)
            .with_name(name)
            .with_option(
                ConnectionProfile::FILE,
                scratch.join(file).display().to_string(),
            )
    };
    DataSourceFile::new(data.join("data-sources.json"))
        .save(&[
            profile("Prod", "prod.sqlite3"),
            profile("Dev", "dev.sqlite3"),
        ])
        .unwrap();
    let (_workspace, mut cx) = start(&data, cx);
    let sources = cx.update(|_, cx| DataSources::global(cx).read(cx).items().to_vec());
    for (source, rows) in sources.iter().zip([
        "(1, 'Ada'), (2, 'Grace'), (3, 'Linus')",
        "(2, 'Grace'), (3, 'Linus T.'), (4, 'Ken')",
    ]) {
        let task = cx.update(|_, cx| {
            source.read(cx).run_statements(
                vec![
                    "create table people (id integer primary key, name text)".into(),
                    format!("insert into people values {rows}"),
                ],
                cx,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        while !task.is_ready() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
            cx.run_until_parked();
        }
        cx.update(|_, cx| {
            source.update(cx, |source, cx| {
                source.request(
                    crate::datasource::CatalogRequest::Objects("main".into()),
                    cx,
                )
            })
        });
    }
    wait_until(&mut cx, "both catalogs", |cx| {
        sources.iter().all(|source| {
            source
                .read(cx)
                .loaded_schema("main")
                .and_then(|schema| schema.relations())
                .is_some_and(|relations| relations.len() == 1)
        })
    });

    let people = crate::objects::ObjectRef::new(
        sources[0].clone(),
        crate::objects::ObjectPath::relation("main", "people"),
    );
    let panel = cx.update(|window, cx| {
        cx.new(|cx| crate::data_compare::DataComparePanel::new(Some(&people), window, cx))
    });
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.compare(window, cx)));
    wait_until(&mut cx, "the comparison", |cx| {
        panel.read(cx).result(cx).is_some()
    });
    let (diff, script) = cx.update(|_, cx| panel.read(cx).result(cx).unwrap());
    assert_eq!(
        (
            diff.only_in_source().len(),
            diff.only_in_target().len(),
            diff.changed().len()
        ),
        (1, 1, 1)
    );
    assert!(script.contains("INSERT INTO"), "{script}");
    assert!(script.contains("UPDATE"), "{script}");
    assert!(script.contains("DELETE FROM"), "{script}");
}
