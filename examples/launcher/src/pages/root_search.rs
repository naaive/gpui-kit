use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

use gpui_kit::{AppContext as _, Context, SharedString, Task, Window};

use super::Page;
use crate::{
    extensions::Catalog,
    model::{Item, ItemId, ListModel, PageModel, Section},
    search::{Score, UsageStore, now, score_item, write_snapshot},
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

/// Where the root search reads the user's applications and usage from.
#[derive(Clone, Debug, Default)]
pub struct RootSearchOptions {
    usage_path: Option<PathBuf>,
    application_directories: Vec<PathBuf>,
    platform_commands: bool,
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
    system: Collection,
    fallbacks: Vec<FallbackCommand>,
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
}

impl RootSearchPage {
    pub fn new(catalog: &Catalog) -> Self {
        Self::with_options(catalog, RootSearchOptions::for_build())
    }

    /// Nothing is read from disk here: usage and applications load when the
    /// page is first shown, so creating the window stays fast.
    pub fn with_options(catalog: &Catalog, options: RootSearchOptions) -> Self {
        Self {
            applications: Collection::new(&Applications::new(&[])),
            extensions: Collection::new(&ExtensionCommands::new(catalog)),
            system: Collection::new(&SystemCommands::new(options.platform_commands)),
            fallbacks: FallbackCommand::from_catalog(catalog),
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
    fn collections(&self) -> [&Collection; 3] {
        [&self.applications, &self.extensions, &self.system]
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
        let recent: Vec<&Item> = self
            .usage
            .most_frecent(now)
            .into_iter()
            .filter_map(|(id, _)| by_id.get(id).copied())
            .take(RECENT_ITEMS)
            .collect();
        let recent_ids: HashSet<&ItemId> = recent.iter().map(|item| item.id()).collect();
        let list = match recent.is_empty() {
            true => list,
            false => list.with_section(
                Section::new()
                    .with_title("Recent")
                    .with_items(recent.into_iter().cloned()),
            ),
        };
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
                                .filter(|item| !recent_ids.contains(item.id()))
                                .cloned(),
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
                let score = score_item(query, item)?;
                let rank = match remembered == Some(item.id().as_str()) {
                    true => f64::INFINITY,
                    false => rank(score, self.usage.frecency(item.id().as_str(), now)),
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
        let list = match matches.is_empty() {
            true => list,
            false => list.with_section(
                Section::new()
                    .with_title("Results")
                    .with_items(matches.into_iter().map(|(_, item)| item.clone())),
            ),
        };
        match fallback::section(query, &self.fallbacks) {
            Some(section) => list.with_section(section),
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
        if query.trim().is_empty() && self.is_stale() {
            self.rescan(cx);
        }
        self.query = query.to_owned();
        self.invalidate(cx);
    }

    /// Learns from the pick. Only the page's own items are remembered; the
    /// calculator's answer and the fallback commands change with every query.
    fn did_perform(&mut self, item: &ItemId, query: &str, cx: &mut Context<Self>) {
        self.start(cx);
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
                "com.gpui-kit.links/links",
                "com.gpui-kit.links/checklist",
                "# System",
                "system/toggle-appearance",
                "system/settings",
                "system/extensions",
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
                "fallback/google"
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
