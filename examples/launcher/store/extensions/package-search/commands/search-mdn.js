// Searches MDN Web Docs as the user types and opens the page chosen.
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  Detail,
  List,
  ListItem,
  MetadataLabel,
  MetadataLink,
} from "launcher";
import { SearchView, getJson, plain } from "../lib/search-view.js";

const SITE = "https://developer.mozilla.org";
const SEARCH_URL = `${SITE}/api/v1/search`;

/** `/en-US/docs/Web/CSS/flex` → `Web › CSS`. */
function section(path) {
  const parts = path.replace(/^\/[^/]+\/docs\//, "").split("/");
  return parts.slice(0, Math.min(2, Math.max(parts.length - 1, 1))).join(" › ").replace(/_/g, " ");
}

function toDocument(document) {
  const path = document.mdn_url ?? "";
  return {
    id: path || document.slug,
    title: document.title ?? path,
    summary: (document.summary ?? "").trim(),
    section: section(path),
    url: `${SITE}${path}`,
  };
}

function detail(document) {
  return new Detail(`# ${plain(document.title)}\n\n${plain(document.summary)}`).children([
    new MetadataLabel("Section", document.section || "MDN"),
    new MetadataLink("Page", document.url.replace(/^https:\/\//, ""), document.url),
  ]);
}

export default class SearchMdn extends SearchView {
  get registry() {
    return "MDN";
  }

  get placeholder() {
    return "Search MDN Web Docs…";
  }

  get prompt() {
    return ["Search MDN Web Docs", "Type an element, property, API or guide, such as flexbox or fetch."];
  }

  nothing(query) {
    return ["No pages found", `Nothing on MDN matches “${query}”.`];
  }

  async search(query) {
    const { documents } = await getJson(`${SEARCH_URL}?q=${encodeURIComponent(query)}&locale=en-US`, "MDN");
    return (documents ?? []).map(toDocument);
  }

  row(document) {
    const item = new ListItem(document.id, document.title).icon("book-open").subtitle(document.summary);
    return (document.section ? item.accessory(document.section) : item)
      .detail(detail(document))
      .actions(
        new ActionPanel().children([
          new Action("Open in Browser").icon("external-link").open_url(document.url),
          new Action("Copy URL").icon("copy").shortcut("secondary-shift-c").copy(document.url),
          new ActionPanelSection("More").children([
            new Action("Copy as Markdown Link").icon("link").shortcut("secondary-shift-l").copy(`[${document.title}](${document.url})`),
            new Action("Copy Title").icon("copy").copy(document.title),
          ]),
        ]),
      );
  }

  render() {
    return this.page(new List());
  }
}
