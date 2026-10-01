// Searches the web with suggestions as the user types. The typed text is
// always the first row, so Enter searches for exactly that; suggestions from
// the chosen engine follow. A pause in typing starts the request, and an
// answer that arrives after a newer query is dropped. Offered as a fallback
// when the root search finds nothing.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  List,
  ListDropdown,
  ListDropdownItem,
  ListItem,
  ListSection,
} from "launcher";
import { close_main_window, launch, open, show_toast } from "launcher/api";
import { ENGINES, engineById } from "../lib/engines.js";
import {
  forgetAllSearches,
  forgetSearch,
  lastEngine,
  recentSearches,
  rememberEngine,
  rememberSearch,
} from "../lib/history.js";

const TYPING_PAUSE_MS = 300;

export default class SearchWeb extends View {
  init(_props, cx) {
    const { arguments: args, preferences } = launch();
    const remembered = preferences.remember_engine !== false ? lastEngine() : null;
    this.engine = engineById(remembered ?? preferences.default_engine ?? "google");
    this.suggestions = [];
    this.recent = recentSearches();
    this.loading = false;
    this.error = null;
    this.generation = 0;
    this.pending = null;
    // Opened as a fallback, or with the argument filled in: suggest at once.
    this.query = (args.query ?? "").trim();
    if (this.query) this.suggest(cx);
  }

  typed(query, cx) {
    this.query = query.trim();
    this.pending?.cancel();
    this.generation += 1;
    this.error = null;
    this.suggestions = [];
    this.loading = this.query !== "";
    this.pending = this.query ? cx.timer.after(TYPING_PAUSE_MS, (timer) => this.suggest(timer)) : null;
    cx.notify();
  }

  chooseEngine(id, cx) {
    this.engine = engineById(id);
    rememberEngine(this.engine.id);
    this.typed(this.query, cx);
  }

  suggest(cx) {
    const run = ++this.generation;
    const query = this.query;
    const engine = this.engine;
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        const suggestions = await engine.suggest(query);
        if (run !== this.generation) return;
        this.suggestions = suggestions.filter((each) => each.title.toLowerCase() !== query.toLowerCase() || each.url);
        this.error = null;
      } catch (error) {
        if (run !== this.generation) return;
        this.suggestions = [];
        this.error = String(error?.message ?? error);
      } finally {
        if (run === this.generation) {
          this.loading = false;
          task.notify();
        }
      }
    });
  }

  /** Opens the results page (or `url`) and remembers the search. */
  go(query, engine, url) {
    try {
      rememberSearch(query, engine.id);
      rememberEngine(engine.id);
    } catch (_) {
      // Searching matters more than remembering.
    }
    open(url ?? engine.search_url(query));
    close_main_window();
  }

  actions(query, engine, url = null) {
    const results = engine.search_url(query);
    const others = ENGINES.filter((other) => other.id !== engine.id).map((other) =>
      new Action(`Search ${other.title}`).icon(other.icon).run(() => this.go(query, other)),
    );
    return new ActionPanel().children([
      url
        ? new Action("Open Article").icon("book-open").run(() => this.go(query, engine, url))
        : new Action(`Search ${engine.title}`).icon("arrow-up-right").run(() => this.go(query, engine)),
      new Action("Copy Search URL").icon("copy").shortcut("secondary-shift-c").copy(url ?? results),
      new Action("Copy Text").icon("copy").shortcut("secondary-shift-t").copy(query),
      ...(url ? [new Action(`Search ${engine.title}`).icon("search").run(() => this.go(query, engine))] : []),
      new ActionPanelSection("Search With").children(others),
    ]);
  }

  recentRows() {
    return this.recent.map((each) => {
      const engine = engineById(each.engine);
      return new ListItem(`recent-${each.engine}-${each.query}`, each.query)
        .icon("rotate-ccw-clock")
        .accessory(engine.title)
        .accessory_date(new Date(each.time).toISOString())
        .actions(
          this.actions(each.query, engine).child(
            new ActionPanelSection("Recent Searches").children([
              new Action("Remove from Recent Searches")
                .icon("x")
                .shortcut("secondary-shift-d")
                .run((cx) => {
                  forgetSearch(each.query, each.engine);
                  this.recent = recentSearches();
                  cx.notify();
                }),
              new Action("Clear Recent Searches")
                .icon("trash")
                .destructive()
                .confirm("Clear recent searches?", "The list of recent searches is emptied.")
                .run((cx) => {
                  forgetAllSearches();
                  this.recent = [];
                  show_toast({ title: "Recent searches cleared", style: "success" });
                  cx.notify();
                }),
            ]),
          ),
        );
    });
  }

  render() {
    const engine = this.engine;
    const dropdown = new ListDropdown("Search Engine")
      .value(engine.id)
      .children(ENGINES.map((each) => new ListDropdownItem(each.id, each.title)))
      .on_change((value, cx) => this.chooseEngine(value, cx));
    const list = new List()
      .placeholder(`Search ${engine.title}…`)
      .loading(this.loading)
      .dropdown(dropdown)
      .on_query_change((query, cx) => this.typed(query, cx));
    if (!this.query) {
      return list
        .empty_title(`Search ${engine.title}`)
        .empty_description("Type to see suggestions; Enter opens the results in your browser.")
        .children(this.recent.length > 0 ? [new ListSection("Recent Searches").children(this.recentRows())] : []);
    }
    const typed = new ListItem("typed", this.query)
      .icon(engine.icon)
      .subtitle(`Search ${engine.title}`)
      .actions(this.actions(this.query, engine));
    const suggestions = this.suggestions.map((each, index) => {
      const item = new ListItem(`suggestion-${index}-${each.title}`, each.title).icon(each.url ? "book-open" : "search");
      return (each.url ? item.subtitle("Wikipedia article") : item).actions(this.actions(each.title, engine, each.url ?? null));
    });
    const problem = this.error
      ? [
          new ListItem("suggestions-unavailable", "Suggestions Unavailable")
            .icon("circle-alert")
            .icon_tone("warning")
            .subtitle(this.error)
            .actions(this.actions(this.query, engine)),
        ]
      : [];
    return list
      .empty_title(`Search ${engine.title}`)
      .empty_description("Type to see suggestions.")
      .children([typed, ...(suggestions.length > 0 ? [new ListSection("Suggestions").children(suggestions)] : problem)]);
  }
}
