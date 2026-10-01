use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

use gpui_kit::{AppContext as _, Context, SharedString, Subscription, Task, Window};

use super::Page;
use crate::{
    customizations,
    extensions::Catalog,
    model::{
        Accessory, Action, ActionEntry, ActionSection, Effect, Image, Item, ItemId, ListModel,
        PageModel, PushHandler, RunHandler, Section, Tone,
    },
    quicklinks::{self, Quicklink},
    script_commands,
    search::{Score, UsageStore, now, score_item, write_snapshot},
    snippets,
    sources::{
        CommandSource, ExtensionCommands,
        applications::{self, Application, Applications},
        calculator,
        fallback::{self, FallbackCommand},
        system::SystemCommands,
    },
};

/// How many used items the empty query shows under "Recent".
const RECENT_ITEMS: usize = 5;
/// Frecency is added to the match score as `WEIGHT × ln(1 + frecency)`, capped,
/// so use reorders comparable matches (an item used daily gains about 20
/// points) without lifting a poor match over a good one.
const FRECENCY_WEIGHT: f64 = 10.0;
const FRECENCY_CAP: f64 = 30.0;
/// Installers write many files in a burst; one rescan follows the burst.
const RESCAN_DELAY: Duration = Duration::from_millis(750);
/// Without a file watcher, returning to an empty query rescans when the last
/// scan is older than this, so a newly installed application appears the
/// next time the launcher is summoned.
const STALE_SCAN: Duration = Duration::from_secs(60);

/// The applications found by the latest scan, shared by every root page.
static LAST_SCAN: std::sync::Mutex<Vec<Application>> = std::sync::Mutex::new(Vec::new());

/// Where the root search reads the user's applications and usage from.
#[derive(Clone, Debug, Default)]
pub struct RootSearchOptions {
    usage_path: Option<PathBuf>,
    application_directories: Vec<PathBuf>,
    platform_commands: bool,
    script_directory: Option<PathBuf>,
}

impl RootSearchOptions {
    /// Nothing from the machine: no usage file, no applications, only the
    /// launcher's own system commands.
    pub fn isolated() -> Self {
        Self::default()
    }

    /// The user's own usage file, applications and system commands.
    pub fn system() -> Self {
        let options = Self::isolated()
            .with_application_directories(applications::default_directories())
            .with_platform_commands(true);
        let options = Self {
            script_directory: script_commands::directory(),
            ..options
        };
        match UsageStore::default_path() {
            Some(path) => options.with_usage_path(path),
            None => options,
        }
    }

    /// What [`RootSearchPage::new`] uses: [`Self::system`], except in this
    /// crate's tests, where every window test would otherwise read and write
    /// the user's real usage file and list whatever happens to be installed.
    fn for_build() -> Self {
        match cfg!(test) {
            true => Self::isolated(),
            false => Self::system(),
        }
    }

    pub fn with_usage_path(mut self, path: PathBuf) -> Self {
        self.usage_path = Some(path);
        self
    }

    pub fn with_application_directories(mut self, directories: Vec<PathBuf>) -> Self {
        self.application_directories = directories;
        self
    }

    pub fn with_platform_commands(mut self, platform_commands: bool) -> Self {
        self.platform_commands = platform_commands;
        self
    }
}

/// The bottom of the navigation stack: every application and command,
/// searchable.
///
/// It is an ordinary List page, drawn by the same renderer as an extension's,
/// so there is no second list implementation to keep in step. It ranks items
/// itself (match score, frecency and the query memory), so the list it
/// returns is not filtered again.
pub struct RootSearchPage {
    options: RootSearchOptions,
    applications: Collection,
    extensions: Collection,
    quicklinks: Collection,
    /// The saved quicklinks, for the ones offered with the query.
    quicklink_data: Vec<Quicklink>,
    snippets: Collection,
    scripts: Collection,
    system: Collection,
    window_layouts: Collection,
    settings_pages: Collection,
    /// What the user customized, copied from the store when it changes.
    aliases: HashMap<String, String>,
    favorites: Vec<String>,
    hotkeys: HashMap<String, String>,
    fallbacks: Vec<FallbackCommand>,
    /// The meeting about to start, offered above everything else.
    next_meeting: Option<crate::calendar::Occurrence>,
    /// The running focus session, with its controls.
    focus_session: Option<Item>,
    /// Reminders overdue or due within the hour.
    due_reminders: Vec<Item>,
    /// The subtitles extension commands had before `update_command_metadata`
    /// replaced them, by command id.
    manifest_subtitles: HashMap<String, Option<SharedString>>,
    usage: UsageStore,
    query: String,
    /// The list for the current query and data; rebuilt only when one of them
    /// changes, because the window asks for the model on every frame.
    list: Option<ListModel>,
    started: bool,
    scanning: bool,
    scanned_at: Option<Instant>,
    scan_task: Option<Task<()>>,
    watch_task: Option<Task<()>>,
    watcher: Option<notify::RecommendedWatcher>,
    /// The latest usage write; the next write waits for it, so writes land in
    /// order.
    save_task: Option<Task<()>>,
    store_subscriptions: Vec<Subscription>,
}

impl RootSearchPage {
    pub fn new(catalog: &Catalog) -> Self {
        Self::with_options(catalog, RootSearchOptions::for_build())
    }

    /// Nothing is read from disk here: usage and applications load when the
    /// page is first shown, so creating the window stays fast.
    pub fn with_options(catalog: &Catalog, options: RootSearchOptions) -> Self {
        // One source, three sections: the launcher's and the system's
        // commands, the window layouts, and the system settings' pages.
        let mut system = Collection::new(&SystemCommands::new(options.platform_commands));
        let mut take = |prefix: &str, title: &str| Collection {
            title: title.to_owned().into(),
            items: system
                .items
                .extract_if(.., |item| item.id().as_str().starts_with(prefix))
                .collect(),
        };
        let window_layouts = take("window/", "Window Management");
        let settings_pages = take("settings/", "System Settings");
        Self {
            // The last scan, so a root page opened again (the window is
            // recreated each time off macOS) lists applications at once.
            applications: Collection::new(&Applications::new(
                match options.application_directories.is_empty() {
                    true => Vec::new(),
                    false => LAST_SCAN
                        .lock()
                        .map(|scan| scan.clone())
                        .unwrap_or_default(),
                }
                .as_slice(),
            )),
            extensions: Collection::new(&ExtensionCommands::new(catalog)),
            quicklinks: Collection {
                title: "Quicklinks".into(),
                items: Vec::new(),
            },
            quicklink_data: Vec::new(),
            snippets: Collection {
                title: "Snippets".into(),
                items: Vec::new(),
            },
            scripts: Collection {
                title: "Script Commands".into(),
                items: Vec::new(),
            },
            aliases: HashMap::new(),
            favorites: Vec::new(),
            hotkeys: HashMap::new(),
            system,
            window_layouts,
            settings_pages,
            fallbacks: FallbackCommand::from_catalog(catalog),
            next_meeting: None,
            focus_session: None,
            due_reminders: Vec::new(),
            manifest_subtitles: HashMap::new(),
            usage: UsageStore::in_memory(),
            query: String::new(),
            list: None,
            started: false,
            scanning: false,
            scanned_at: None,
            scan_task: None,
            watch_task: None,
            watcher: None,
            save_task: None,
            store_subscriptions: Vec::new(),
            options,
        }
    }

    /// Loads usage and starts scanning applications, once.
    fn start(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        // A few kilobytes of JSON: reading it here is cheaper than a round
        // trip to the background, and the first list is then ranked right.
        if let Some(path) = &self.options.usage_path {
            self.usage = UsageStore::load(path.clone());
        }
        if !self.options.application_directories.is_empty() {
            self.rescan(cx);
            self.watch(cx);
        }
        if let Some(store) = quicklinks::store(cx) {
            let saved = store.read(cx).quicklinks().to_vec();
            self.set_quicklinks(saved, cx);
            self.store_subscriptions
                .push(cx.observe(&store, |page, store, cx| {
                    let saved = store.read(cx).quicklinks().to_vec();
                    page.set_quicklinks(saved, cx);
                }));
        }
        self.rescan_scripts();
        if let Some(store) = customizations::store(cx) {
            self.read_customizations(&store, cx);
            self.store_subscriptions
                .push(cx.observe(&store, |page, store, cx| {
                    page.read_customizations(&store, cx);
                    page.invalidate(cx);
                }));
        }
        if self.options.platform_commands && crate::focus::is_supported() {
            let focus = crate::focus::focus(cx);
            self.focus_session = crate::focus::session_item(cx);
            self.store_subscriptions
                .push(cx.observe(&focus, |page, _, cx| {
                    let session = crate::focus::session_item(cx);
                    // The clock ticks every second; only a shown change
                    // rebuilds the list.
                    if session.as_ref().map(Item::accessories)
                        != page.focus_session.as_ref().map(Item::accessories)
                        || session.as_ref().map(Item::title)
                            != page.focus_session.as_ref().map(Item::title)
                    {
                        page.focus_session = session;
                        page.invalidate(cx);
                    }
                }));
        }
        if self.options.platform_commands {
            let schedule = crate::calendar::schedule(cx);
            self.read_schedule(&schedule, cx);
            self.store_subscriptions
                .push(cx.observe(&schedule, |page, schedule, cx| {
                    page.read_schedule(&schedule, cx);
                    page.invalidate(cx);
                }));
        }
        if self.options.platform_commands {
            let reminders = crate::reminders::store(cx);
            self.read_reminders(&reminders, cx);
            self.store_subscriptions
                .push(cx.observe(&reminders, |page, reminders, cx| {
                    page.read_reminders(&reminders, cx);
                    page.invalidate(cx);
                }));
        }
        if let Some(store) = snippets::store(cx) {
            self.snippets.items = snippets::snippet_items(store.read(cx).snippets());
            self.store_subscriptions
                .push(cx.observe(&store, |page, store, cx| {
                    page.snippets.items = snippets::snippet_items(store.read(cx).snippets());
                    page.invalidate(cx);
                }));
        }
    }

    fn read_schedule(
        &mut self,
        schedule: &gpui_kit::Entity<crate::calendar::Schedule>,
        cx: &mut Context<Self>,
    ) {
        self.next_meeting = schedule.read(cx).next_meeting().cloned();
    }

    fn read_reminders(
        &mut self,
        reminders: &gpui_kit::Entity<crate::reminders::Reminders>,
        cx: &mut Context<Self>,
    ) {
        self.due_reminders = reminders
            .read(cx)
            .due_soon()
            .iter()
            .map(crate::reminders::due_item)
            .collect();
    }

    /// Reads the script commands folder again; a few small files.
    fn rescan_scripts(&mut self) {
        if let Some(directory) = &self.options.script_directory {
            self.scripts.items =
                script_commands::script_items(&script_commands::discover(directory));
        }
    }

    fn read_customizations(
        &mut self,
        store: &gpui_kit::Entity<customizations::Customizations>,
        cx: &mut Context<Self>,
    ) {
        let store = store.read(cx);
        self.aliases = store
            .aliases()
            .map(|(item, alias)| (item.to_owned(), alias.to_owned()))
            .collect();
        self.hotkeys = store
            .hotkeys()
            .map(|(item, shortcut)| (item.to_owned(), shortcut.to_owned()))
            .collect();
        self.favorites = store.favorites().to_vec();
    }

    /// An item as the root search shows it: with its alias and hotkey, and
    /// the actions that customize it.
    fn present(&self, item: &Item) -> Item {
        let id = item.id().as_str().to_owned();
        let title = item.title().clone();
        let favorite = self.favorites.contains(&id);
        let item = match self.hotkeys.get(&id) {
            Some(hotkey) => item
                .clone()
                .with_leading_accessory(Accessory::text(hotkey.clone()).with_tooltip("Hotkey")),
            None => item.clone(),
        };
        let item = match self.aliases.get(&id) {
            Some(alias) => item.with_leading_accessory(
                Accessory::tag(alias.clone(), Tone::Neutral).with_tooltip("Alias"),
            ),
            None => item,
        };
        let toggle = {
            let id = id.clone();
            Action::new(
                match favorite {
                    true => "Remove from Favorites",
                    false => "Add to Favorites",
                },
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    if let Some(store) = customizations::store(cx) {
                        store.update(cx, |store, cx| store.set_favorite(&id, !favorite, cx));
                    }
                })),
            )
            .with_image(Image::Icon(
                match favorite {
                    true => "star-off",
                    false => "star",
                }
                .into(),
            ))
        };
        let section = ActionSection::new()
            .with_title("Customize")
            .with_entry(ActionEntry::Action(toggle));
        let moves = [
            ("Move Up in Favorites", -1, "arrow-up"),
            ("Move Down in Favorites", 1, "arrow-down"),
        ];
        let section = match favorite {
            true => moves
                .into_iter()
                .fold(section, |section, (title, step, icon)| {
                    let id = id.clone();
                    section.with_entry(ActionEntry::Action(
                        Action::new(
                            title,
                            Effect::Run(RunHandler::new(move |(), _, cx| {
                                if let Some(store) = customizations::store(cx) {
                                    store
                                        .update(cx, |store, cx| store.move_favorite(&id, step, cx));
                                }
                            })),
                        )
                        .with_image(Image::Icon(icon.into())),
                    ))
                }),
            false => section,
        };
        let alias = {
            let (id, title) = (id.clone(), title.clone());
            Action::new(
                "Set Alias…",
                Effect::Push(PushHandler::new(move |_, cx| {
                    customizations::alias_page(id.clone(), title.clone(), cx)
                })),
            )
            .with_image(Image::Icon("at-sign".into()))
        };
        let hotkey = Action::new(
            "Set Hotkey…",
            Effect::Push(PushHandler::new(move |_, cx| {
                customizations::hotkey_page(id.clone(), title.clone(), cx)
            })),
        )
        .with_image(Image::Icon("keyboard".into()));
        let section = section
            .with_entry(ActionEntry::Action(alias))
            .with_entry(ActionEntry::Action(hotkey));
        let section = match self.deeplink(&item) {
            Some(link) => section.with_entry(ActionEntry::Action(
                Action::new("Copy Deeplink", Effect::Copy(link.into()))
                    .with_image(Image::Icon("link-2".into())),
            )),
            None => section,
        };
        let actions = item.actions().clone().with_section(section);
        item.with_actions(actions)
    }

    /// The `launcher://` link that opens `item`, for the items that have one.
    fn deeplink(&self, item: &Item) -> Option<String> {
        let id = item.id().as_str();
        if self
            .extensions
            .items
            .iter()
            .any(|known| known.id() == item.id())
        {
            return Some(format!("launcher://extensions/{id}"));
        }
        let (kind, name) = id.split_once('/')?;
        let extension = crate::sources::system::BUILT_IN_EXTENSION;
        match kind {
            // The launcher's own commands read best by their short name.
            "system" => Some(format!("launcher://extensions/{extension}/{name}")),
            // Others by their full id, its `/` escaped into one segment.
            "window" | "script" | "settings" | "quicklink" | "snippet" => Some(format!(
                "launcher://extensions/{extension}/{}",
                percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC)
            )),
            _ => None,
        }
    }

    fn set_quicklinks(&mut self, saved: Vec<Quicklink>, cx: &mut Context<Self>) {
        self.quicklinks.items = quicklinks::quicklink_items(&saved);
        self.quicklink_data = saved;
        self.invalidate(cx);
    }

    /// Scans applications in the background. A scan still running is
    /// cancelled, so a stale result never replaces a newer one.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        let directories = self.options.application_directories.clone();
        self.scanning = true;
        self.scan_task = Some(cx.spawn(async move |this, cx| {
            let applications = cx
                .background_spawn(async move { applications::scan(&directories) })
                .await;
            this.update(cx, |page, cx| page.set_applications(&applications, cx))
                .ok();
        }));
    }

    fn set_applications(&mut self, applications: &[Application], cx: &mut Context<Self>) {
        if let Ok(mut scan) = LAST_SCAN.lock() {
            *scan = applications.to_vec();
        }
        self.applications = Collection::new(&Applications::new(applications));
        self.scanning = false;
        self.scanned_at = Some(Instant::now());
        self.invalidate(cx);
    }

    /// Rescans after the application directories change.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let (changed, changes) = smol::channel::unbounded::<()>();
        self.watcher = applications::watch(&self.options.application_directories, move || {
            changed.try_send(()).ok();
        });
        if self.watcher.is_none() {
            return;
        }
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            while changes.recv().await.is_ok() {
                cx.background_executor().timer(RESCAN_DELAY).await;
                while changes.try_recv().is_ok() {}
                if this.update(cx, |page, cx| page.rescan(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn is_stale(&self) -> bool {
        self.watcher.is_none()
            && !self.scanning
            && self
                .scanned_at
                .is_some_and(|scanned_at| scanned_at.elapsed() > STALE_SCAN)
    }

    /// Shows `subtitle` for an extension command in place of its manifest's,
    /// as the extension asked through `update_command_metadata`; `None`
    /// restores the manifest's.
    pub fn set_command_subtitle(
        &mut self,
        command: &crate::extensions::CommandId,
        subtitle: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = command.to_string();
        // An extension's command, or one of the launcher's own, such as the
        // Extension Store saying it has updates.
        let Some(item) = self
            .extensions
            .items
            .iter_mut()
            .chain(self.system.items.iter_mut())
            .find(|item| item.id().as_str() == id)
        else {
            return;
        };
        let manifest = self
            .manifest_subtitles
            .entry(id)
            .or_insert_with(|| item.subtitle().cloned());
        let subtitle = subtitle.or_else(|| manifest.clone());
        if item.subtitle() == subtitle.as_ref() {
            return;
        }
        *item = item.clone().with_subtitle(subtitle.unwrap_or_default());
        self.invalidate(cx);
    }

    /// Lists the commands of another catalog, after an extension was
    /// installed or removed.
    pub fn set_catalog(&mut self, catalog: &Catalog, cx: &mut Context<Self>) {
        self.extensions = Collection::new(&ExtensionCommands::new(catalog));
        self.fallbacks = FallbackCommand::from_catalog(catalog);
        self.invalidate(cx);
    }

    fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.list = None;
        cx.notify();
    }

    /// Writes usage on the background executor, after any earlier write.
    fn save(&mut self, now: u64, cx: &mut Context<Self>) {
        let Some((path, json)) = self.usage.snapshot(now) else {
            return;
        };
        let previous = self.save_task.take();
        self.save_task = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            if let Err(error) = write_snapshot(&path, &json) {
                tracing::warn!("cannot write usage to {}: {error}", path.display());
            }
        }));
    }

    /// The query-independent collections, in the order the empty query lists
    /// them and ties in a search keep.
    fn collections(&self) -> [&Collection; 8] {
        [
            &self.applications,
            &self.extensions,
            &self.quicklinks,
            &self.snippets,
            &self.scripts,
            &self.system,
            &self.window_layouts,
            &self.settings_pages,
        ]
    }

    fn items(&self) -> impl Iterator<Item = &Item> {
        self.collections()
            .into_iter()
            .flat_map(|collection| collection.items.iter())
    }

    fn contains(&self, id: &ItemId) -> bool {
        self.items().any(|item| item.id() == id)
    }

    fn build_list(&self, query: &str, now: u64) -> ListModel {
        let list = ListModel::new()
            .with_placeholder("Search for apps and commands…")
            .with_filtering(false)
            .with_loading(self.scanning && self.scanned_at.is_none());
        let query = query.trim();
        match query.is_empty() {
            true => self.browse(list, now),
            false => self.search(list, query, now),
        }
    }

    /// With no query: what was used recently, then each collection under its
    /// own title. A recent item is not repeated below, because the list keeps
    /// the selection by item id.
    fn browse(&self, list: ListModel, now: u64) -> ListModel {
        let by_id: HashMap<&str, &Item> = self
            .items()
            .map(|item| (item.id().as_str(), item))
            .collect();
        let favorites: Vec<&Item> = self
            .favorites
            .iter()
            .filter_map(|id| by_id.get(id.as_str()).copied())
            .collect();
        let favorite_ids: HashSet<&ItemId> = favorites.iter().map(|item| item.id()).collect();
        let recent: Vec<&Item> = self
            .usage
            .most_frecent(now)
            .into_iter()
            .filter_map(|(id, _)| by_id.get(id).copied())
            .filter(|item| !favorite_ids.contains(item.id()))
            .take(RECENT_ITEMS)
            .collect();
        let mut shown: HashSet<&ItemId> = recent.iter().map(|item| item.id()).collect();
        shown.extend(favorite_ids);
        let list = match &self.focus_session {
            Some(session) => list.with_section(
                Section::new()
                    .with_title("Focus")
                    .with_item(session.clone()),
            ),
            None => list,
        };
        let list = match &self.next_meeting {
            Some(meeting) => {
                list.with_section(Section::new().with_title("Upcoming Meeting").with_item(
                    crate::calendar::occurrence_item(meeting, chrono::Local::now()),
                ))
            }
            None => list,
        };
        let list = match self.due_reminders.is_empty() {
            true => list,
            false => list.with_section(
                Section::new()
                    .with_title("Reminders")
                    .with_items(self.due_reminders.iter().cloned()),
            ),
        };
        let list = [("Favorites", favorites), ("Recent", recent)]
            .into_iter()
            .filter(|(_, items)| !items.is_empty())
            .fold(list, |list, (title, items)| {
                list.with_section(
                    Section::new()
                        .with_title(title)
                        .with_items(items.into_iter().map(|item| self.present(item))),
                )
            });
        self.collections()
            .into_iter()
            .filter(|collection| !collection.items.is_empty())
            .fold(list, |list, collection| {
                list.with_section(
                    Section::new()
                        .with_title(collection.title.clone())
                        .with_items(
                            collection
                                .items
                                .iter()
                                .filter(|item| !shown.contains(item.id()))
                                .map(|item| self.present(item)),
                        ),
                )
            })
    }

    /// With a query: the calculator's answer, one ranked list of matches, then
    /// what else to do with the query. One list rather than one per
    /// collection, because grouping would split the best match from the next
    /// best.
    fn search(&self, list: ListModel, query: &str, now: u64) -> ListModel {
        let remembered = self.usage.remembered(query);
        let mut matches: Vec<(f64, &Item)> = self
            .items()
            .filter_map(|item| {
                let id = item.id().as_str();
                // An alias typed exactly wins, whether or not the title
                // matches; then the item picked last time for this query;
                // then a keyword typed exactly, such as a snippet's.
                let is_alias = self
                    .aliases
                    .get(id)
                    .is_some_and(|alias| alias.eq_ignore_ascii_case(query));
                let score = match is_alias {
                    true => 0,
                    false => score_item(query, item)?,
                };
                let rank = if is_alias {
                    f64::INFINITY
                } else if remembered == Some(id) {
                    f64::MAX
                } else if item
                    .keywords()
                    .iter()
                    .any(|keyword| keyword.eq_ignore_ascii_case(query))
                {
                    f64::MAX / 2.
                } else {
                    rank(score, self.usage.frecency(id, now))
                };
                Some((rank, item))
            })
            .collect();
        // Stable, so equal ranks keep collection order.
        matches.sort_by(|(a, _), (b, _)| b.total_cmp(a));

        let list = match calculator::item(query) {
            Some(answer) => {
                list.with_section(Section::new().with_title("Calculator").with_item(answer))
            }
            None => list,
        };
        let list = match crate::colors::item(query) {
            Some(color) => list.with_section(Section::new().with_title("Color").with_item(color)),
            None => list,
        };
        let list = match matches.is_empty() {
            true => list,
            false => list.with_section(
                Section::new()
                    .with_title("Results")
                    .with_items(matches.into_iter().map(|(_, item)| self.present(item))),
            ),
        };
        match fallback::section(query, &self.fallbacks) {
            Some(section) => list.with_section(
                section.with_items(quicklinks::fallback_items(&self.quicklink_data, query)),
            ),
            None => list,
        }
    }
}

/// A source's commands, under the title the empty query lists them with.
struct Collection {
    title: SharedString,
    items: Vec<Item>,
}

impl Collection {
    fn new(source: &impl CommandSource) -> Self {
        Self {
            title: source.title(),
            items: source.commands(),
        }
    }
}

/// An item's place in the results: how well it matched, plus how much it has
/// been used.
fn rank(score: Score, frecency: f64) -> f64 {
    f64::from(score) + (FRECENCY_WEIGHT * frecency.ln_1p()).min(FRECENCY_CAP)
}

impl Page for RootSearchPage {
    /// A page above may have created a script, quicklink or snippet.
    fn did_reappear(&mut self, cx: &mut Context<Self>) {
        self.rescan_scripts();
        // A custom window layout may have been created, edited or deleted.
        if self.options.platform_commands {
            self.window_layouts.items = crate::window_layout::commands();
        }
        self.invalidate(cx);
    }

    fn title(&self) -> SharedString {
        "Launcher".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        self.start(cx);
        let list = match self.list.take() {
            Some(list) => list,
            None => self.build_list(&self.query, now()),
        };
        self.list = Some(list.clone());
        PageModel::List(list)
    }

    fn set_query(&mut self, query: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.start(cx);
        if query.trim().is_empty() {
            if self.options.platform_commands {
                let schedule = crate::calendar::schedule(cx);
                // Meetings come and go with the clock, not only with fetches.
                self.read_schedule(&schedule, cx);
                schedule.update(cx, |schedule, cx| schedule.refresh(false, cx));
            }
            self.rescan_scripts();
            if self.is_stale() {
                self.rescan(cx);
            }
        }
        self.query = query.to_owned();
        self.invalidate(cx);
    }

    /// Learns from the pick. Only the page's own items are remembered; the
    /// calculator's answer and the fallback commands change with every query.
    fn did_perform(&mut self, item: &ItemId, query: &str, cx: &mut Context<Self>) {
        self.start(cx);
        if self.options.platform_commands && item.as_str() == calculator::RESULT_ID {
            crate::calculator_history::record(query);
        }
        if !self.contains(item) {
            return;
        }
        let now = now();
        self.usage.record(item.as_str(), query, now);
        self.save(now, cx);
        self.invalidate(cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::{model::Effect, sources::applications::Launch};

    fn bundled() -> Catalog {
        Catalog::discover(&[PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions")])
    }

    fn application(name: &str) -> Application {
        Application::new(name, PathBuf::from(format!("/apps/{name}")), Launch::Open)
    }

    /// A page with `names` as its applications, as a finished scan leaves it.
    fn page(names: &[&str]) -> RootSearchPage {
        let mut page = RootSearchPage::with_options(&bundled(), RootSearchOptions::isolated());
        let applications: Vec<Application> = names.iter().map(|name| application(name)).collect();
        page.applications = Collection::new(&Applications::new(&applications));
        page
    }

    fn rows(page: &RootSearchPage, query: &str, now: u64) -> Vec<String> {
        let list = page.build_list(query, now);
        list.sections()
            .iter()
            .flat_map(|section| {
                std::iter::once(format!("# {}", section.title().unwrap())).chain(
                    section
                        .items()
                        .iter()
                        .map(|item| item.id().as_str().to_owned()),
                )
            })
            .collect()
    }

    const NOW: u64 = 1_700_000_000;

    #[test]
    fn test_empty_query_lists_each_collection_under_its_title() {
        let mut page = page(&["Safari", "Terminal"]);
        assert_eq!(
            rows(&page, "", NOW),
            [
                "# Applications",
                "app:/apps/Safari",
                "app:/apps/Terminal",
                "# Extensions",
                "com.gpui-kit.github/search-repositories",
                "com.gpui-kit.github/my-pull-requests",
                "com.gpui-kit.github/my-issues",
                "com.gpui-kit.github/notifications",
                "com.gpui-kit.github/unread-notifications",
                "com.gpui-kit.links/links",
                "com.gpui-kit.links/checklist",
                "com.gpui-kit.links/search-docs",
                "com.gpui-kit.links/copy-date",
                "com.gpui-kit.links/tray-links",
                "com.gpui-kit.links/weekend",
                "# System",
                "system/search-files",
                "system/create-quicklink",
                "system/search-quicklinks",
                "system/create-snippet",
                "system/search-snippets",
                "system/search-processes",
                "system/create-script-command",
                "system/script-commands-folder",
                "system/search-bookmarks",
                "system/clipboard-history",
                "system/toggle-appearance",
                "system/settings",
                "system/extensions",
                "system/store",
                "system/quit",
            ]
        );

        page.usage.record("app:/apps/Terminal", "", NOW);
        page.usage.record("system/quit", "", NOW - 1);
        page.usage.record("app:/apps/Uninstalled", "", NOW);
        assert_eq!(
            rows(&page, "", NOW)[..4],
            [
                "# Recent",
                "app:/apps/Terminal",
                "system/quit",
                "# Applications"
            ],
            "recent items come first, are not repeated, and must still exist"
        );
        assert_eq!(
            rows(&page, "", NOW)
                .iter()
                .filter(|row| row.as_str() == "app:/apps/Terminal")
                .count(),
            1
        );
    }

    #[test]
    fn test_frecency_and_the_query_memory_reorder_results() {
        let mut page = page(&["Terminal", "Terminator"]);
        assert_eq!(
            rows(&page, "term", NOW)[..3],
            ["# Results", "app:/apps/Terminal", "app:/apps/Terminator"]
        );

        page.usage.record("app:/apps/Terminator", "", NOW);
        assert_eq!(
            rows(&page, "term", NOW)[1],
            "app:/apps/Terminator",
            "use breaks a tie in match score"
        );
        assert_eq!(
            rows(&page, "term", NOW + 90 * 24 * 60 * 60)[1],
            "app:/apps/Terminal",
            "old use fades"
        );

        // A weaker match that was picked for this exact query comes first.
        page.usage.record("system/toggle-appearance", "te", NOW);
        assert_eq!(rows(&page, "te", NOW)[1], "system/toggle-appearance");
        assert_eq!(rows(&page, "TE ", NOW)[1], "system/toggle-appearance");
    }

    #[test]
    fn test_query_adds_calculator_and_fallback_sections() {
        let page = page(&["Safari"]);
        let rows = rows(&page, "2 + 2", NOW);
        assert_eq!(
            rows,
            [
                "# Calculator",
                "calculator/result",
                "# Use “2 + 2” with…",
                "fallback/google",
                "fallback/com.gpui-kit.github/search-repositories",
                "fallback/com.gpui-kit.links/search-docs"
            ]
        );

        let list = page.build_list("gpui kit", NOW);
        let fallback = list.sections().last().unwrap();
        let Effect::OpenUrl(url) = fallback.items()[0].primary_action().unwrap().effect() else {
            panic!("the web search opens a URL");
        };
        assert_eq!(url.as_ref(), "https://www.google.com/search?q=gpui%20kit");
    }

    #[test]
    fn test_aliases_keywords_and_favorites() {
        let mut page = page(&["Terminal", "Settings Sync"]);
        page.aliases.insert("system/quit".into(), "q".into());
        page.aliases
            .insert("app:/apps/Terminal".into(), "zz".into());
        assert_eq!(
            rows(&page, "zz", NOW)[1],
            "app:/apps/Terminal",
            "an alias matches even when the title does not"
        );
        page.usage.record("app:/apps/Terminal", "q", NOW);
        assert_eq!(
            rows(&page, "Q", NOW)[1],
            "system/quit",
            "an exact alias beats the remembered pick"
        );
        assert_eq!(
            rows(&page, "exit", NOW)[1],
            "system/quit",
            "an exact keyword comes first"
        );

        page.favorites = vec!["system/settings".into(), "app:/apps/Terminal".into()];
        let browse = rows(&page, "", NOW);
        assert_eq!(
            browse[..3],
            ["# Favorites", "system/settings", "app:/apps/Terminal"]
        );
        assert_eq!(
            browse
                .iter()
                .filter(|row| *row == "system/settings")
                .count(),
            1,
            "a favorite is not listed again"
        );
        let list = page.build_list("", NOW);
        let quit = list
            .items()
            .find(|item| item.id().as_str() == "system/quit")
            .unwrap();
        assert!(
            quit.accessories()[0].tone().is_some(),
            "the alias leads as a tag"
        );
        assert!(
            quit.actions()
                .all_actions()
                .any(|action| action.title() == "Set Hotkey…")
        );
    }

    #[test]
    fn test_pinyin_titles_are_found() {
        let page = page(&["微信", "系统设置", "Safari"]);
        assert_eq!(rows(&page, "wx", NOW)[1], "app:/apps/微信");
        assert_eq!(rows(&page, "xtsz", NOW)[1], "app:/apps/系统设置");
    }

    #[gpui::test]
    fn test_a_pick_is_remembered_across_launches(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("usage.json");
        let options = RootSearchOptions::isolated().with_usage_path(path.clone());
        let catalog = bundled();
        let page = cx.new(|_| RootSearchPage::with_options(&catalog, options.clone()));
        let quit = ItemId::new("system/quit");
        page.update(cx, |page, cx| {
            page.did_perform(&quit, "q", cx);
            // Not one of the page's own items: not remembered.
            page.did_perform(&ItemId::new(calculator::RESULT_ID), "1+1", cx);
        });
        cx.run_until_parked();

        let reopened = cx.new(|_| RootSearchPage::with_options(&catalog, options));
        reopened.update(cx, |page, cx| {
            page.start(cx);
            assert_eq!(page.usage.remembered("q"), Some("system/quit"));
            assert!(page.usage.frecency("system/quit", now()) > 0.9);
            assert_eq!(page.usage.frecency(calculator::RESULT_ID, now()), 0.0);
            assert_eq!(
                page.build_list("", now()).sections()[0]
                    .title()
                    .unwrap()
                    .as_ref(),
                "Recent"
            );
        });
    }
}
