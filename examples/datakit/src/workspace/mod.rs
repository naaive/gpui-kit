//! The main window: the dock with the explorer, consoles and history, the
//! title bar with its menus, and the status bar.

mod start_panel;
#[cfg(test)]
mod tests;

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc, time::Duration};

use anyhow::{Context as _, Result};
use datakit_driver::DataSourceId;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    GlobalState, Sizable as _, Theme, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{
        ClosePanel, DockArea, DockAreaState, DockEvent, DockLayout, DockPlacement, DockSkin,
        ToggleZoom, panel_handle, register_panel,
    },
    h_flex,
    input::{Copy, Cut, Paste, Redo, SelectAll, Undo},
    menu::AppMenuBar,
    notification::Notification,
    status_bar::StatusBar,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, KeyBinding, Menu, MenuItem, MouseButton, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, WeakEntity, Window, WindowBounds, WindowOptions, actions, div,
    prelude::FluentBuilder as _, px, rems, size,
};
use rust_i18n::t;

use crate::{
    compare::ComparePanel,
    console::{
        CancelExecution, Commit, ConsolePanel, ExecuteStatement, ExplainAnalyze, ExplainPlan,
        FormatSql, Rollback, SaveConsoleAs, Sessions, ShowLocalHistory,
    },
    data_compare::DataComparePanel,
    datasource::{DataSource, DataSourceForm, DataSources},
    designer::TableDesigner,
    diagram::DiagramPanel,
    dump::DumpDialog,
    explorer::{ExplorerEvent, ExplorerPanel},
    files::{FilesEvent, FilesPanel},
    history::{HistoryPanel, HistoryPanelEvent},
    import::ImportDialog,
    navigation::{Navigation, NavigationEvent},
    objects::{ObjectPath, ObjectRef},
    recent::{Recent, RecentEntry, RecentEvent, RecentFiles as RecentFilesList},
    search::{SearchEvent, SearchEverywhere as SearchDialog, SearchScope, SearchableAction},
    services::Services,
    services_panel::ServicesPanel,
    settings::{Settings, SettingsDialog},
    table_editor::TablePanel,
};

use start_panel::StartPanel;

actions!(
    workspace,
    [
        /// Add a data source.
        NewDataSource,
        /// Open a console for the selected data source.
        NewConsole,
        /// Show or hide the database explorer.
        ToggleExplorer,
        /// Show or hide the query history.
        ToggleHistory,
        /// Put every tool window back where it started.
        ResetLayout,
        /// Find any object or command.
        SearchEverywhere,
        /// Find an object by name and open it.
        GoToObject,
        /// Find a command by what it does.
        FindAction,
        /// Choose the appearance, language and SQL style.
        OpenSettings,
        /// Open SQL files in consoles.
        OpenSqlFile,
        /// Add a folder of SQL files to the Files window.
        AttachFolder,
        /// Show or hide the Files window.
        ToggleFiles,
        /// Choose among the SQL files and tables opened lately.
        RecentFiles,
        Quit
    ]
);

/// Bumped when the default layout changes in a way a saved one cannot
/// express; a saved layout of another version is replaced.
const LAYOUT_VERSION: usize = 1;

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-shift-l", NewConsole, None),
        KeyBinding::new("alt-1", ToggleExplorer, None),
        KeyBinding::new("alt-8", ToggleHistory, None),
        KeyBinding::new("shift-escape", ToggleZoom, None),
        KeyBinding::new("secondary-w", ClosePanel, None),
        KeyBinding::new("secondary-k", SearchEverywhere, None),
        KeyBinding::new("secondary-n", GoToObject, None),
        KeyBinding::new("secondary-e", RecentFiles, None),
        KeyBinding::new("secondary-shift-a", FindAction, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        KeyBinding::new("secondary-o", OpenSqlFile, None),
        // Windows and Linux close the window with the system's own keys.
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    // Keys are bound first: menus read the keymap when they are built.
    set_menus(cx);
}

/// Name the menus in the current language.
fn set_menus(cx: &mut App) {
    cx.set_menus(menus());
    // Windows and Linux draw the same menus in the title bar.
    GlobalState::global_mut(cx)
        .set_app_menus(menus().into_iter().map(|menu| menu.owned()).collect());
}

fn menus() -> Vec<Menu> {
    let mut file = vec![
        MenuItem::action(t!("menu.new_data_source").to_string(), NewDataSource),
        MenuItem::action(t!("menu.new_console").to_string(), NewConsole),
        MenuItem::Separator,
        MenuItem::action(t!("menu.open_sql_file").to_string(), OpenSqlFile),
        MenuItem::action(t!("menu.attach_folder").to_string(), AttachFolder),
        MenuItem::action(t!("console.save_as").to_string(), SaveConsoleAs),
        MenuItem::action(t!("menu.local_history").to_string(), ShowLocalHistory),
        MenuItem::Separator,
        MenuItem::action(t!("menu.close_tab").to_string(), ClosePanel),
    ];
    let mut menus = Vec::new();
    if cfg!(target_os = "macos") {
        // The first menu of a Mac app is the application menu.
        menus.push(Menu {
            name: "DataKit".into(),
            items: vec![
                MenuItem::action(t!("menu.settings").to_string(), OpenSettings),
                MenuItem::Separator,
                MenuItem::action(t!("menu.quit").to_string(), Quit),
            ],
            disabled: false,
        });
    } else {
        file.extend([
            MenuItem::Separator,
            MenuItem::action(t!("menu.settings").to_string(), OpenSettings),
            MenuItem::Separator,
            MenuItem::action(t!("menu.exit").to_string(), Quit),
        ]);
    }
    menus.extend([
        Menu {
            name: t!("menu.file").to_string().into(),
            items: file,
            disabled: false,
        },
        Menu {
            name: t!("menu.edit").to_string().into(),
            items: vec![
                MenuItem::action(t!("menu.undo").to_string(), Undo),
                MenuItem::action(t!("menu.redo").to_string(), Redo),
                MenuItem::Separator,
                MenuItem::action(t!("menu.cut").to_string(), Cut),
                MenuItem::action(t!("menu.copy").to_string(), Copy),
                MenuItem::action(t!("menu.paste").to_string(), Paste),
                MenuItem::action(t!("menu.select_all").to_string(), SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: t!("menu.view").to_string().into(),
            items: vec![
                MenuItem::action(t!("menu.explorer").to_string(), ToggleExplorer),
                MenuItem::action(t!("menu.files").to_string(), ToggleFiles),
                MenuItem::action(t!("menu.history").to_string(), ToggleHistory),
                MenuItem::Separator,
                MenuItem::action(t!("menu.reset_layout").to_string(), ResetLayout),
            ],
            disabled: false,
        },
        Menu {
            name: t!("menu.navigate").to_string().into(),
            items: vec![
                MenuItem::action(t!("menu.search_everywhere").to_string(), SearchEverywhere),
                MenuItem::action(t!("menu.go_to_object").to_string(), GoToObject),
                MenuItem::action(t!("menu.find_action").to_string(), FindAction),
                MenuItem::action(t!("menu.recent_files").to_string(), RecentFiles),
            ],
            disabled: false,
        },
        Menu {
            name: t!("menu.query").to_string().into(),
            items: vec![
                MenuItem::action(t!("menu.execute").to_string(), ExecuteStatement),
                MenuItem::action(t!("menu.cancel").to_string(), CancelExecution),
                MenuItem::Separator,
                MenuItem::action(t!("menu.explain").to_string(), ExplainPlan),
                MenuItem::action(t!("menu.explain_analyze").to_string(), ExplainAnalyze),
                MenuItem::Separator,
                MenuItem::action(t!("menu.format").to_string(), FormatSql),
                MenuItem::Separator,
                MenuItem::action(t!("menu.commit").to_string(), Commit),
                MenuItem::action(t!("menu.rollback").to_string(), Rollback),
            ],
            disabled: false,
        },
    ]);
    menus
}

pub fn open_main_window(cx: &mut App) -> Result<()> {
    let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // Below this the explorer, an editor and its results stop fitting.
        window_min_size: Some(size(px(720.), px(480.))),
        ..TitleBar::window_options()
    };
    let (window, _) = gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title("DataKit");
        cx.new(|cx| Workspace::new(window, cx))
    })?;
    window.update(cx, |_, window, _| window.activate_window())?;
    Ok(())
}

pub struct Workspace {
    dock_area: Entity<DockArea>,
    explorer: Entity<ExplorerPanel>,
    files: Entity<FilesPanel>,
    history: Entity<HistoryPanel>,
    services: Entity<ServicesPanel>,
    start: Entity<StartPanel>,
    app_menu_bar: Entity<AppMenuBar>,
    console_counts: HashMap<DataSourceId, usize>,
    /// The data editors open, so opening a table again shows its editor.
    tables: Rc<RefCell<Vec<WeakEntity<TablePanel>>>>,
    last_layout: Option<DockAreaState>,
    save_layout_task: Option<Task<()>>,
    search: Option<Entity<SearchDialog>>,
    /// The Recent Files list while it is open; its events need it alive.
    recent: Option<Entity<RecentFilesList>>,
    search_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dock_area, _skin) = DockSkin::dock_area("datakit", Some(LAYOUT_VERSION), window, cx);
        let explorer = cx.new(|cx| ExplorerPanel::new(window, cx));
        let files = cx.new(|cx| FilesPanel::new(window, cx));
        let history = cx.new(|cx| HistoryPanel::new(window, cx));
        let services = cx.new(ServicesPanel::new);
        let start = cx.new(StartPanel::new);
        let app_menu_bar = AppMenuBar::new(cx);
        app_menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));

        // A saved layout names panels; these teach the dock to build them.
        // The tool windows are single instances the workspace owns.
        register_panel(cx, ExplorerPanel::NAME, {
            let explorer = explorer.clone();
            move |context, _, cx| {
                let state = context.state().clone();
                explorer.update(cx, |explorer, cx| explorer.restore(&state, cx));
                panel_handle(explorer.clone())
            }
        });
        register_panel(cx, HistoryPanel::NAME, {
            let history = history.clone();
            move |_, _, _| panel_handle(history.clone())
        });
        register_panel(cx, FilesPanel::NAME, {
            let files = files.clone();
            move |_, _, _| panel_handle(files.clone())
        });
        register_panel(cx, ServicesPanel::NAME, {
            let services = services.clone();
            move |_, _, _| panel_handle(services.clone())
        });
        register_panel(cx, StartPanel::NAME, {
            let start = start.clone();
            move |_, _, _| panel_handle(start.clone())
        });
        register_panel(cx, ConsolePanel::NAME, |context, window, cx| {
            let state = context.state().clone();
            panel_handle(cx.new(|cx| ConsolePanel::restore(&state, window, cx)))
        });
        register_panel(cx, ComparePanel::NAME, |_, window, cx| {
            panel_handle(cx.new(|cx| ComparePanel::new(None, window, cx)))
        });
        register_panel(cx, DataComparePanel::NAME, |_, window, cx| {
            panel_handle(cx.new(|cx| DataComparePanel::new(None, window, cx)))
        });
        register_panel(cx, DiagramPanel::NAME, {
            let start = start.clone();
            move |context, _, cx| {
                let found = DiagramPanel::saved(context.state()).and_then(|(id, schema)| {
                    let data_source = DataSources::global(cx).read(cx).get(&id, cx)?;
                    Some((data_source, schema))
                });
                match found {
                    Some((data_source, schema)) => {
                        panel_handle(cx.new(|cx| DiagramPanel::new(data_source, schema, cx)))
                    }
                    // Its data source was removed.
                    None => panel_handle(start.clone()),
                }
            }
        });
        let tables: Rc<RefCell<Vec<WeakEntity<TablePanel>>>> = Rc::default();
        register_panel(cx, TablePanel::NAME, {
            let tables = tables.clone();
            move |context, window, cx| {
                let state = context.state().clone();
                let table = cx.new(|cx| TablePanel::restore(&state, window, cx));
                tables.borrow_mut().push(table.downgrade());
                panel_handle(table)
            }
        });

        let subscriptions = vec![
            cx.subscribe_in(
                &explorer,
                window,
                |this, _, event: &ExplorerEvent, window, cx| {
                    this.on_explorer_event(event, window, cx)
                },
            ),
            cx.subscribe_in(&files, window, |this, _, event: &FilesEvent, window, cx| {
                let FilesEvent::Open(path) = event;
                this.open_file(path.clone(), window, cx);
            }),
            cx.subscribe_in(
                &history,
                window,
                |this, _, event: &HistoryPanelEvent, window, cx| {
                    let HistoryPanelEvent::Open { data_source, sql } = event;
                    let found = DataSources::global(cx).read(cx).get(data_source, cx);
                    if let Some(found) = found {
                        this.open_console(found, Some(sql.to_string()), false, window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &dock_area,
                window,
                |this, _, event: &DockEvent, window, cx| {
                    if let DockEvent::LayoutChanged = event {
                        this.keep_center_occupied(window, cx);
                        this.save_layout(cx);
                    }
                },
            ),
            cx.subscribe_in(
                &Navigation::global(cx),
                window,
                |this, _, event: &NavigationEvent, window, cx| match event {
                    NavigationEvent::Reveal(object) => this.reveal(object, window, cx),
                    NavigationEvent::Open(object) => this.open_object(object, window, cx),
                    NavigationEvent::ShowConsole { console, range } => {
                        let id = panel_handle(console.clone()).panel_id(cx);
                        this.dock_area
                            .update(cx, |area, cx| area.select_panel(id, window, cx));
                        console.update(cx, |console, cx| console.select(range.clone(), window, cx));
                    }
                    NavigationEvent::OpenConsole { object, sql } => this.open_console(
                        object.data_source().clone(),
                        Some(sql.clone()),
                        false,
                        window,
                        cx,
                    ),
                },
            ),
            window.observe_window_appearance(|window, cx| {
                if Settings::global(cx).follows_system_appearance() {
                    Theme::sync_system_appearance(Some(window), cx);
                }
            }),
            // A new language renames the menus; a new appearance recolors
            // the window.
            cx.observe_global_in::<Settings>(window, |this, window, cx| {
                set_menus(cx);
                this.app_menu_bar
                    .update(cx, |menu_bar, cx| menu_bar.reload(cx));
                Settings::apply_appearance(window, cx);
                cx.notify();
            }),
        ];

        let mut workspace = Self {
            dock_area,
            explorer,
            files,
            history,
            services,
            start,
            app_menu_bar,
            console_counts: HashMap::new(),
            tables,
            last_layout: None,
            save_layout_task: None,
            search: None,
            recent: None,
            search_subscription: None,
            _subscriptions: subscriptions,
        };
        // Keys work from the start: something in the window has focus.
        let focus = workspace.explorer.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        if let Err(error) = workspace.load_layout(window, cx) {
            tracing::info!("starting with the default layout: {error:#}");
            workspace.reset_layout(&ResetLayout, window, cx);
        }
        cx.on_app_quit({
            let dock_area = workspace.dock_area.clone();
            move |_, cx| {
                let state = dock_area.read(cx).dump(cx);
                let path = layout_path(cx);
                cx.background_executor().spawn(async move {
                    if let Err(error) = write_layout(&path, &state) {
                        tracing::error!("{error:#}");
                    }
                })
            }
        })
        .detach();
        Settings::apply_appearance(window, cx);
        workspace
    }

    fn load_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<()> {
        let path = layout_path(cx);
        let json = std::fs::read_to_string(&path)
            .with_context(|| format!("no layout at {}", path.display()))?;
        let state: DockAreaState = serde_json::from_str(&json)?;
        anyhow::ensure!(
            state.version == Some(LAYOUT_VERSION),
            "the saved layout is from another version"
        );
        self.dock_area.update(cx, |area, cx| {
            area.load(state.clone(), window, cx)?;
            for placement in [DockPlacement::Left, DockPlacement::Bottom] {
                area.set_dock_collapsible(placement, true, window, cx);
            }
            anyhow::Ok(())
        })?;
        // New consoles are numbered after the ones the layout brings back.
        self.console_counts.clear();
        let mut saved = vec![&state.center];
        saved.extend(
            [&state.left_dock, &state.bottom_dock, &state.right_dock]
                .into_iter()
                .flatten()
                .map(|dock| dock.panel()),
        );
        while let Some(panel) = saved.pop() {
            if let Some(data_source) = ConsolePanel::saved_data_source(panel) {
                *self.console_counts.entry(data_source).or_default() += 1;
            }
            saved.extend(&panel.children);
        }
        self.last_layout = Some(state);
        // A layout saved before a tool window existed gets it where the
        // default layout puts it.
        let missing = [
            (panel_handle(self.services.clone()), DockPlacement::Bottom),
            (panel_handle(self.files.clone()), DockPlacement::Left),
        ];
        self.dock_area.update(cx, |area, cx| {
            for (panel, placement) in missing {
                if area.panel(panel.panel_id(cx)).is_none() {
                    area.add_panel_view(panel, placement, None, window, cx);
                }
            }
        });
        self.keep_center_occupied(window, cx);
        Ok(())
    }

    fn reset_layout(&mut self, _: &ResetLayout, window: &mut Window, cx: &mut Context<Self>) {
        let center = DockLayout::v_split().child(
            DockLayout::tabs().panel_view(panel_handle(self.start.clone()), cx),
            None,
        );
        let left = DockLayout::v_split().child(
            DockLayout::tabs()
                .panel_view(panel_handle(self.explorer.clone()), cx)
                .panel_view(panel_handle(self.files.clone()), cx),
            None,
        );
        let bottom = DockLayout::v_split().child(
            DockLayout::tabs()
                .panel_view(panel_handle(self.history.clone()), cx)
                .panel_view(panel_handle(self.services.clone()), cx),
            None,
        );
        let rem = window.rem_size();
        self.dock_area.update(cx, |area, cx| {
            area.set_center(center, window, cx);
            area.set_dock(DockPlacement::Left, left, window, cx);
            area.set_dock_size(DockPlacement::Left, rems(18.).to_pixels(rem), window, cx);
            area.set_dock(DockPlacement::Bottom, bottom, window, cx);
            area.set_dock_size(DockPlacement::Bottom, rems(14.).to_pixels(rem), window, cx);
            for placement in [DockPlacement::Left, DockPlacement::Bottom] {
                area.set_dock_collapsible(placement, true, window, cx);
            }
            // History is a click away; the editor gets the room until then.
            if area.is_dock_open(DockPlacement::Bottom) {
                area.toggle_dock(DockPlacement::Bottom, window, cx);
            }
        });
        self.console_counts.clear();
    }

    /// Show the start panel whenever the last console closes, and only then.
    fn keep_center_occupied(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start = self.start.clone();
        self.dock_area.update(cx, |area, cx| {
            if area.is_empty(DockPlacement::Center, cx) {
                area.add_panel_view(panel_handle(start), DockPlacement::Center, None, window, cx);
            }
        });
    }

    /// Save the layout a moment after it stops changing.
    fn save_layout(&mut self, cx: &mut Context<Self>) {
        self.save_layout_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let Ok(Some((path, state))) = this.update(cx, |this, cx| {
                let state = this.dock_area.read(cx).dump(cx);
                if this.last_layout.as_ref() == Some(&state) {
                    return None;
                }
                this.last_layout = Some(state.clone());
                Some((layout_path(cx), state))
            }) else {
                return;
            };
            let written = cx
                .background_spawn(async move { write_layout(&path, &state) })
                .await;
            if let Err(error) = written {
                tracing::error!("{error:#}");
            }
        }));
    }

    fn open_console(
        &mut self,
        data_source: Entity<DataSource>,
        sql: Option<String>,
        run: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = data_source.read(cx).profile().id().clone();
        let count = self.console_counts.entry(id).or_insert(0);
        *count += 1;
        let base = data_source.read(cx).name();
        let name: SharedString = if *count == 1 {
            base
        } else {
            format!("{base} ({count})").into()
        };
        let console = cx.new(|cx| {
            ConsolePanel::new(
                &data_source,
                name,
                sql.as_deref().unwrap_or_default(),
                window,
                cx,
            )
        });
        let start = self.start.clone();
        self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(
                panel_handle(console.clone()),
                DockPlacement::Center,
                None,
                window,
                cx,
            );
            area.remove_panel(start, window, cx);
        });
        if run && let Some(sql) = sql {
            console.update(cx, |console, cx| console.run_sql(&sql, window, cx));
        }
        let focus = console.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn on_explorer_event(
        &mut self,
        event: &ExplorerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ExplorerEvent::OpenConsole {
                data_source,
                sql,
                run,
            } => self.open_console(data_source.clone(), sql.clone(), *run, window, cx),
            ExplorerEvent::NewDataSource => DataSourceForm::open(None, window, cx),
            ExplorerEvent::Open(object) => self.open_object(object, window, cx),
            ExplorerEvent::EditTable {
                data_source,
                schema,
                relation,
            } => TableDesigner::open(data_source, schema.clone(), relation.clone(), window, cx),
            ExplorerEvent::CompareSchema {
                data_source,
                schema,
            } => {
                let source = Some((data_source.clone(), schema.clone()));
                let panel = cx.new(|cx| ComparePanel::new(source, window, cx));
                self.add_center_panel(panel_handle(panel), window, cx);
            }
            ExplorerEvent::ShowDiagram {
                data_source,
                schema,
            } => {
                let panel = cx.new(|cx| DiagramPanel::new(data_source.clone(), schema.clone(), cx));
                self.add_center_panel(panel_handle(panel), window, cx);
            }
            ExplorerEvent::ImportData(object) => ImportDialog::open(object, window, cx),
            ExplorerEvent::CompareData(object) => {
                let panel = cx.new(|cx| DataComparePanel::new(Some(object), window, cx));
                self.add_center_panel(panel_handle(panel), window, cx);
            }
            ExplorerEvent::DumpOrRestore(data_source) => DumpDialog::open(data_source, window, cx),
        }
    }

    /// Open `object`: a relation's rows, or any other object's DDL.
    pub fn open_object(&mut self, object: &ObjectRef, window: &mut Window, cx: &mut Context<Self>) {
        let data_source = object.data_source().clone();
        match object.path() {
            ObjectPath::Relation { .. } => self.open_table(object, window, cx),
            _ => {
                let ddl = object.ddl(cx);
                self.open_console(data_source, ddl, false, window, cx);
            }
        }
    }

    /// Show the data editor of the relation `object`, opening one when
    /// there is none.
    fn open_table(&mut self, object: &ObjectRef, window: &mut Window, cx: &mut Context<Self>) {
        Recent::record(
            RecentEntry::Table {
                data_source: object.data_source().read(cx).profile().id().clone(),
                schema: object.path().schema().to_string(),
                relation: object.path().name().to_string(),
            },
            cx,
        );
        self.tables
            .borrow_mut()
            .retain(|table| table.upgrade().is_some());
        let existing = self
            .tables
            .borrow()
            .iter()
            .filter_map(|table| table.upgrade())
            .find(|table| table.read(cx).shows(object, cx));
        let table = match existing {
            Some(table) => {
                let id = panel_handle(table.clone()).panel_id(cx);
                self.dock_area
                    .update(cx, |area, cx| area.select_panel(id, window, cx));
                table
            }
            None => {
                let table = cx.new(|cx| TablePanel::new(object, window, cx));
                self.tables.borrow_mut().push(table.downgrade());
                let start = self.start.clone();
                self.dock_area.update(cx, |area, cx| {
                    area.add_panel_view(
                        panel_handle(table.clone()),
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    );
                    area.remove_panel(start, window, cx);
                });
                table
            }
        };
        let focus = table.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    /// Put `panel` in the center, in place of the start panel.
    fn add_center_panel(
        &mut self,
        panel: std::sync::Arc<dyn gpui_kit::base::dock::PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = self.start.clone();
        self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(panel, DockPlacement::Center, None, window, cx);
            area.remove_panel(start, window, cx);
        });
    }

    /// Ask for SQL files and open each in a console.
    fn open_sql_file(&mut self, _: &OpenSqlFile, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                for path in paths {
                    this.open_file(path, window, cx);
                }
            });
        })
        .detach();
    }

    /// Show the SQL file at `path` in a console: the one already editing it,
    /// or a new one run against the selected data source.
    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        Recent::record(RecentEntry::File { path: path.clone() }, cx);
        let existing = Sessions::global(cx)
            .read(cx)
            .consoles()
            .into_iter()
            .find(|console| console.read(cx).file() == Some(&path));
        if let Some(console) = existing {
            let id = panel_handle(console.clone()).panel_id(cx);
            self.dock_area
                .update(cx, |area, cx| area.select_panel(id, window, cx));
            let focus = console.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            return;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                window.push_notification(
                    Notification::error(
                        t!(
                            "files.open_failed",
                            path = path.display().to_string(),
                            error = error.to_string()
                        )
                        .to_string(),
                    ),
                    cx,
                );
                return;
            }
        };
        let data_source = self.explorer.read(cx).selected_data_source(cx);
        let console =
            cx.new(|cx| ConsolePanel::open_file(path, &text, data_source.as_ref(), window, cx));
        self.add_center_panel(panel_handle(console.clone()), window, cx);
        let focus = console.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    /// Show the explorer with `object` selected.
    fn reveal(&mut self, object: &ObjectRef, window: &mut Window, cx: &mut Context<Self>) {
        self.dock_area.update(cx, |area, cx| {
            if !area.is_dock_open(DockPlacement::Left) {
                area.toggle_dock(DockPlacement::Left, window, cx);
            }
        });
        self.explorer
            .update(cx, |explorer, cx| explorer.reveal(object, window, cx));
    }

    /// Show the Files window in front of the explorer.
    fn show_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = panel_handle(self.files.clone()).panel_id(cx);
        self.dock_area.update(cx, |area, cx| {
            if !area.is_dock_open(DockPlacement::Left) {
                area.toggle_dock(DockPlacement::Left, window, cx);
            }
            area.select_panel(id, window, cx);
        });
    }

    fn search(&mut self, scope: SearchScope, window: &mut Window, cx: &mut Context<Self>) {
        let search = SearchDialog::open(scope, searchable_actions(), window, cx);
        self.search = Some(search.clone());
        self.search_subscription = Some(cx.subscribe_in(
            &search,
            window,
            |this, _, event: &SearchEvent, window, cx| match event {
                SearchEvent::Open(object) => this.open_object(object, window, cx),
            },
        ));
    }

    fn recent_files(&mut self, _: &RecentFiles, window: &mut Window, cx: &mut Context<Self>) {
        let list = RecentFilesList::open(window, cx);
        self.recent = Some(list.clone());
        self.search_subscription = Some(cx.subscribe_in(
            &list,
            window,
            |this, _, event: &RecentEvent, window, cx| match event {
                RecentEvent::Open(RecentEntry::File { path }) => {
                    this.open_file(path.clone(), window, cx)
                }
                RecentEvent::Open(RecentEntry::Table {
                    data_source,
                    schema,
                    relation,
                }) => {
                    if let Some(data_source) = DataSources::global(cx).read(cx).get(data_source, cx)
                    {
                        let object = ObjectRef::new(
                            data_source,
                            ObjectPath::relation(schema.as_str(), relation.as_str()),
                        );
                        this.open_object(&object, window, cx);
                    }
                }
            },
        ));
    }

    fn new_console(&mut self, _: &NewConsole, window: &mut Window, cx: &mut Context<Self>) {
        let data_source = self
            .explorer
            .read(cx)
            .selected_data_source(cx)
            .or_else(|| DataSources::global(cx).read(cx).items().first().cloned());
        match data_source {
            Some(data_source) => self.open_console(data_source, None, false, window, cx),
            None => DataSourceForm::open(None, window, cx),
        }
    }

    fn new_data_source(&mut self, _: &NewDataSource, window: &mut Window, cx: &mut Context<Self>) {
        DataSourceForm::open(None, window, cx);
    }

    fn toggle_dock(
        &mut self,
        placement: DockPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock_area
            .update(cx, |area, cx| area.toggle_dock(placement, window, cx));
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The Mac draws the menus in the system menu bar; elsewhere they
        // take the place of the title.
        TitleBar::new()
            .child(h_flex().gap_2().map(|row| {
                if cfg!(target_os = "macos") {
                    row.child(div().text_sm().child("DataKit"))
                } else {
                    row.child(self.app_menu_bar.clone())
                }
            }))
            .child(
                h_flex()
                    .gap_1()
                    .pr_2()
                    // A press here is a click, not the start of moving the
                    // window.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("title-search")
                            .ghost()
                            .small()
                            .icon(IconName::Search)
                            .tooltip_with_action(
                                t!("menu.search_everywhere").to_string(),
                                &SearchEverywhere,
                                None,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.search(SearchScope::Everything, window, cx)
                            })),
                    )
                    .child(
                        Button::new("title-new-console")
                            .ghost()
                            .small()
                            .icon(IconName::SquareTerminal)
                            .tooltip_with_action(
                                t!("menu.new_console").to_string(),
                                &NewConsole,
                                None,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_console(&NewConsole, window, cx)
                            })),
                    ),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        StatusBar::new()
            .left(
                Button::new("toggle-explorer")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Database)
                    .tooltip_with_action(t!("menu.explorer").to_string(), &ToggleExplorer, None)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_dock(DockPlacement::Left, window, cx)
                    })),
            )
            .left(
                Button::new("toggle-history")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ListClock)
                    .tooltip_with_action(t!("menu.history").to_string(), &ToggleHistory, None)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_dock(DockPlacement::Bottom, window, cx)
                    })),
            )
    }
}

/// The commands Search Everywhere offers, named as the menus name them.
fn searchable_actions() -> Vec<SearchableAction> {
    let action = |label: std::borrow::Cow<'static, str>, action: Box<dyn gpui_kit::Action>| {
        SearchableAction {
            label: label.to_string().into(),
            action,
        }
    };
    vec![
        action(t!("menu.new_data_source"), Box::new(NewDataSource)),
        action(t!("menu.new_console"), Box::new(NewConsole)),
        action(t!("menu.execute"), Box::new(ExecuteStatement)),
        action(t!("menu.cancel"), Box::new(CancelExecution)),
        action(t!("menu.explain"), Box::new(ExplainPlan)),
        action(t!("menu.explain_analyze"), Box::new(ExplainAnalyze)),
        action(t!("menu.format"), Box::new(FormatSql)),
        action(t!("menu.commit"), Box::new(Commit)),
        action(t!("menu.rollback"), Box::new(Rollback)),
        action(t!("menu.explorer"), Box::new(ToggleExplorer)),
        action(t!("menu.history"), Box::new(ToggleHistory)),
        action(t!("menu.reset_layout"), Box::new(ResetLayout)),
        action(t!("menu.go_to_object"), Box::new(GoToObject)),
        action(t!("menu.recent_files"), Box::new(RecentFiles)),
        action(t!("menu.settings"), Box::new(OpenSettings)),
        action(t!("menu.open_sql_file"), Box::new(OpenSqlFile)),
        action(t!("menu.attach_folder"), Box::new(AttachFolder)),
        action(t!("menu.files"), Box::new(ToggleFiles)),
        action(t!("console.save_as"), Box::new(SaveConsoleAs)),
        action(t!("menu.local_history"), Box::new(ShowLocalHistory)),
    ]
}

fn layout_path(cx: &App) -> PathBuf {
    Services::global(cx).data_directory().join("layout.json")
}

fn write_layout(path: &std::path::Path, state: &DockAreaState) -> Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let json = serde_json::to_string_pretty(state)?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, json)?;
    std::fs::rename(&temporary, path).with_context(|| format!("cannot replace {}", path.display()))
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("workspace")
            .size_full()
            .on_action(cx.listener(Self::new_console))
            .on_action(cx.listener(Self::new_data_source))
            .on_action(cx.listener(Self::reset_layout))
            .on_action(cx.listener(|this, _: &SearchEverywhere, window, cx| {
                this.search(SearchScope::Everything, window, cx)
            }))
            .on_action(cx.listener(|this, _: &GoToObject, window, cx| {
                this.search(SearchScope::Objects, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FindAction, window, cx| {
                this.search(SearchScope::Actions, window, cx)
            }))
            .on_action(|_: &OpenSettings, window, cx| SettingsDialog::open(window, cx))
            .on_action(cx.listener(Self::open_sql_file))
            .on_action(cx.listener(Self::recent_files))
            .on_action(cx.listener(|this, _: &AttachFolder, window, cx| {
                this.show_files(window, cx);
                this.files.update(cx, |files, cx| files.attach_folder(cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleFiles, window, cx| this.show_files(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleExplorer, window, cx| {
                this.toggle_dock(DockPlacement::Left, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleHistory, window, cx| {
                this.toggle_dock(DockPlacement::Bottom, window, cx)
            }))
            .child(self.render_title_bar(cx))
            .child(div().flex_1().min_h_0().child(self.dock_area.clone()))
            .child(self.render_status_bar(cx))
    }
}
