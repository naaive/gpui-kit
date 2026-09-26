// A list that searches as the user types. `on_query_change` turns the
// launcher's own filtering off; the command asks GitHub instead, which its
// `gpui-shell.json` allows for one host, one method and one path, and nothing
// else. A pause in typing starts the request, and an answer that arrives after
// a newer query is dropped.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  Detail,
  List,
  ListItem,
  MetadataLabel,
  MetadataLink,
  MetadataSeparator,
  MetadataTags,
} from "launcher";
import { launch, show_toast } from "launcher/api";

const SEARCH_URL = "https://api.github.com/search/repositories";
const TYPING_PAUSE_MS = 350;

function formatCount(count) {
  if (count < 1000) return String(count);
  return `${(count / 1000).toFixed(count < 10000 ? 1 : 0)}k`;
}

async function searchRepositories(query, token) {
  const headers = { Accept: "application/vnd.github+json" };
  if (token) headers.Authorization = `Bearer ${token}`;
  const response = await fetch(`${SEARCH_URL}?q=${encodeURIComponent(query)}&per_page=20`, { headers });
  if (!response.ok) {
    throw new Error(response.status === 403 ? "Rate limit reached; try again in a minute." : `GitHub answered ${response.status}.`);
  }
  const { items } = await response.json();
  return items.map((repository) => ({
    id: String(repository.id),
    name: repository.full_name,
    description: repository.description ?? "",
    url: repository.html_url,
    clone_url: repository.clone_url,
    stars: repository.stargazers_count,
    forks: repository.forks_count,
    language: repository.language,
    topics: repository.topics ?? [],
  }));
}

function detail(repository) {
  const topics = repository.topics.reduce((tags, topic) => tags.tag(topic), new MetadataTags("Topics"));
  return new Detail(`# ${repository.name}\n\n${repository.description}`).children([
    new MetadataLabel("Stars", formatCount(repository.stars)),
    new MetadataLabel("Forks", formatCount(repository.forks)),
    new MetadataLabel("Language", repository.language ?? "None"),
    ...(repository.topics.length > 0 ? [topics] : []),
    new MetadataSeparator(),
    new MetadataLink("Repository", repository.name, repository.url),
  ]);
}

export default class SearchRepositories extends View {
  init(props, cx) {
    const { arguments: args, preferences } = launch();
    this.token = preferences.token ?? "";
    this.repositories = [];
    this.loading = false;
    this.error = null;
    this.generation = 0;
    this.pending = null;
    // Opened as a fallback, or with its argument filled in: search at once.
    this.query = (args.query ?? "").trim();
    if (this.query) this.search(cx);
  }

  typed(query, cx) {
    this.query = query.trim();
    this.pending?.cancel();
    this.generation += 1;
    this.error = null;
    this.loading = this.query !== "";
    if (!this.query) this.repositories = [];
    this.pending = this.query ? cx.timer.after(TYPING_PAUSE_MS, (timer) => this.search(timer)) : null;
    cx.notify();
  }

  search(cx) {
    const run = ++this.generation;
    const query = this.query;
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        const repositories = await searchRepositories(query, this.token);
        if (run !== this.generation) return;
        this.repositories = repositories;
        this.error = null;
      } catch (error) {
        if (run !== this.generation) return;
        this.repositories = [];
        this.error = String(error?.message ?? error);
        show_toast({ title: "Cannot search GitHub", message: this.error, style: "failure" });
      } finally {
        if (run === this.generation) {
          this.loading = false;
          task.notify();
        }
      }
    });
  }

  row(repository) {
    return new ListItem(repository.id, repository.name)
      .icon("book-marked")
      .subtitle(repository.description)
      .accessory(`★ ${formatCount(repository.stars)}`)
      .detail(detail(repository))
      .actions(
        new ActionPanel().children([
          new Action("Open in Browser").open_url(repository.url),
          new Action("Copy Clone URL").icon("copy").shortcut("secondary-shift-c").copy(repository.clone_url),
          new ActionPanelSection("More").children([
            new Action("Open Issues").icon("circle-alert").open_url(`${repository.url}/issues`),
            new Action("Open Pull Requests").icon("git-pull-request").open_url(`${repository.url}/pulls`),
            new Action("Copy Name").copy(repository.name),
          ]),
        ]),
      );
  }

  render() {
    const [title, description] = this.error
      ? ["Cannot search GitHub", this.error]
      : this.query
        ? ["No repositories found", "Try another name, owner or topic."]
        : ["Search GitHub", "Type a repository name, owner or topic."];
    return new List()
      .placeholder("Search repositories…")
      .loading(this.loading)
      .showing_detail(this.repositories.length > 0)
      .empty_title(this.loading ? "Searching…" : title)
      .empty_description(this.loading ? `Looking for “${this.query}”` : description)
      .on_query_change((query, cx) => this.typed(query, cx))
      .children(this.repositories.map((repository) => this.row(repository)));
  }
}
