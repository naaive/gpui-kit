// The search-as-you-type page every registry command shares. A subclass
// names the registry and says how to search it and how to draw one result;
// this class waits for a pause in typing, drops answers to stale queries and
// turns a failed request into the list's empty state.
import { View } from "gpui-kit";
import { launch } from "launcher/api";

const TYPING_PAUSE_MS = 300;

export class SearchView extends View {
  /** The registry's name, as the empty state shows it. */
  get registry() {
    return "the registry";
  }

  /** What the search field asks for. */
  get placeholder() {
    return "Search packages…";
  }

  /** The empty state before anything is typed: `[title, description]`. */
  get prompt() {
    return [`Search ${this.registry}`, "Type a package name or keyword."];
  }

  /** The empty state when a search finds nothing: `[title, description]`. */
  nothing(query) {
    return ["No packages found", `Nothing on ${this.registry} matches “${query}”.`];
  }

  /** Answers a promise of results for `query`; each result has a stable `id`. */
  async search(_query) {
    return [];
  }

  /** One `ListItem` for a result. */
  row(_result) {
    throw new Error("row is not implemented");
  }

  init(_props, cx) {
    this.cx = cx;
    this.results = [];
    this.loading = false;
    this.error = null;
    this.generation = 0;
    this.pending = null;
    // Opened with its argument filled in: search at once.
    this.query = String(launch().arguments.query ?? "").trim();
    if (this.query) this.start();
  }

  typed(query, cx) {
    this.query = query.trim();
    this.pending?.cancel();
    this.generation += 1;
    this.error = null;
    this.loading = this.query !== "";
    if (!this.query) this.results = [];
    this.pending = this.query ? cx.timer.after(TYPING_PAUSE_MS, () => this.start()) : null;
    cx.notify();
  }

  start() {
    const run = ++this.generation;
    const query = this.query;
    this.loading = true;
    this.cx.spawn(async (task) => {
      try {
        const results = await this.search(query);
        if (run !== this.generation) return;
        this.results = results;
        this.error = null;
      } catch (error) {
        if (run !== this.generation) return;
        this.results = [];
        this.error = String(error?.message ?? error);
      } finally {
        if (run === this.generation) {
          this.loading = false;
          task.notify();
        }
      }
    });
  }

  /** Fills `list` in; the command's `render` passes a new `List`. */
  page(list) {
    const [title, description] = this.error
      ? [`Cannot search ${this.registry}`, this.error]
      : this.query
        ? this.nothing(this.query)
        : this.prompt;
    return list
      .placeholder(this.placeholder)
      .loading(this.loading)
      .showing_detail(this.results.length > 0)
      .empty_title(this.loading ? "Searching…" : title)
      .empty_description(this.loading ? `Looking for “${this.query}”` : description)
      .on_query_change((query, cx) => this.typed(query, cx))
      .children(this.results.map((result) => this.row(result)));
  }
}

/** Fetches JSON, turning a failed status into an error that says which. */
export async function getJson(url, registry, headers = {}) {
  const response = await fetch(url, { headers: { Accept: "application/json", ...headers } });
  if (!response.ok) {
    throw new Error(
      response.status === 429 ? "Too many requests; try again in a minute." : `${registry} answered ${response.status}.`,
    );
  }
  return response.json();
}

/** 1234567 as "1.2M". */
export function formatCount(count) {
  if (count == null || Number.isNaN(Number(count))) return "Unknown";
  const value = Number(count);
  if (value < 1000) return String(value);
  if (value < 1000000) return `${(value / 1000).toFixed(value < 10000 ? 1 : 0)}k`;
  if (value < 1000000000) return `${(value / 1000000).toFixed(value < 10000000 ? 1 : 0)}M`;
  return `${(value / 1000000000).toFixed(1)}B`;
}

/** An ISO date as `YYYY-MM-DD`, or `Unknown`. */
export function formatDate(iso) {
  return typeof iso === "string" && iso.length >= 10 ? iso.slice(0, 10) : "Unknown";
}

/** A repository URL as a browser opens it: `git+https://…/x.git` → `https://…/x`. */
export function browsableRepository(url) {
  if (typeof url !== "string" || !url) return null;
  let cleaned = url
    .trim()
    .replace(/^git\+/, "")
    .replace(/^git:\/\//, "https://")
    .replace(/^ssh:\/\/git@/, "https://")
    .replace(/^git@([^:]+):/, "https://$1/")
    .replace(/\.git$/, "");
  if (!/^https?:\/\//.test(cleaned)) return null;
  cleaned = cleaned.replace(/#.*$/, "");
  return cleaned;
}

/** Markdown-safe text: a description must not turn into a heading or a link. */
export function plain(text) {
  return String(text ?? "")
    .replace(/[\\`*_[\]<>#]/g, (character) => `\\${character}`)
    .trim();
}
