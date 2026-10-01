// Searches the pages and databases shared with the integration as you type
// (recently edited ones before typing), with each page's properties and
// first blocks beside the list.
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
import { launch } from "launcher/api";
import { AppendForm } from "../lib/append-form.js";
import { appUrl, describe, pagePreview, propertiesOf, search, titleOf } from "../lib/notion.js";
import { PageForm } from "../lib/page-form.js";
import { itemIcon, label } from "../lib/present.js";

const TYPING_PAUSE_MS = 300;

function propertyMetadata([name, text, type, property]) {
  if (type === "multi_select") {
    return property.multi_select.reduce((tags, option) => tags.tag(option.name), new MetadataTags(name));
  }
  if ((type === "select" || type === "status") && text) return new MetadataTags(name).tag(text);
  if (type === "url" && /^https?:\/\//.test(text)) return new MetadataLink(name, text, text);
  return new MetadataLabel(name, text);
}

export default class SearchNotion extends View {
  init(props, cx) {
    this.results = [];
    this.previews = {};
    this.query = String(launch().arguments.query ?? "").trim();
    this.loading = true;
    this.error = null;
    this.generation = 0;
    this.pending = null;
    this.search(cx);
  }

  typed(query, cx) {
    this.query = query.trim();
    this.pending?.cancel();
    this.generation += 1;
    this.error = null;
    this.loading = true;
    this.pending = cx.timer.after(TYPING_PAUSE_MS, (timer) => this.search(timer));
    cx.notify();
  }

  search(cx) {
    const run = ++this.generation;
    const query = this.query;
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        const results = await search(query);
        if (run !== this.generation) return;
        this.results = results;
        this.error = null;
        if (results[0]) this.preview(results[0].id, task);
      } catch (error) {
        if (run !== this.generation) return;
        this.results = [];
        this.error = describe(error);
      } finally {
        if (run === this.generation) {
          this.loading = false;
          task.notify();
        }
      }
    });
  }

  /** Loads the first blocks of a page once, when it is selected. */
  preview(id, cx) {
    const page = this.results.find((result) => result.id === id);
    if (!page || page.object !== "page" || id in this.previews) return;
    this.previews[id] = null;
    cx.spawn(async (task) => {
      try {
        this.previews[id] = await pagePreview(id);
      } catch (error) {
        this.previews[id] = `_${describe(error)}_`;
      }
      task.notify();
    });
  }

  detail(object) {
    const isPage = object.object === "page";
    const preview = this.previews[object.id];
    const body = !isPage
      ? (object.description ?? []).map((part) => part.plain_text).join("") || "_A database_"
      : preview === undefined || preview === null
        ? "_Loading…_"
        : preview || "_This page is empty._";
    const properties = isPage
      ? propertiesOf(object).map(propertyMetadata)
      : [new MetadataLabel("Properties", Object.keys(object.properties ?? {}).join(", ") || "None")];
    return new Detail(`# ${label(object)}\n\n${body}`).children([
      ...properties,
      ...(properties.length > 0 ? [new MetadataSeparator()] : []),
      new MetadataLabel("Type", isPage ? "Page" : "Database"),
      new MetadataLabel("Edited", String(object.last_edited_time ?? "").slice(0, 16).replace("T", " ")),
      new MetadataLabel("Created", String(object.created_time ?? "").slice(0, 10)),
      new MetadataLink("Link", "notion.so", object.url),
    ]);
  }

  actions(object) {
    const title = titleOf(object);
    const create = new Action(object.object === "database" ? "Add Page to Database…" : "Create Page Inside…")
      .icon("file-plus")
      .shortcut("secondary-n")
      .push(() => new PageForm({ parent: object }), "Create Page");
    return new ActionPanel().children([
      new Action("Open in Notion").icon("external-link").open_url(appUrl(object.url)),
      new Action("Open in Browser").icon("globe").open_url(object.url),
      new ActionPanelSection("Copy").children([
        new Action("Copy URL").icon("link").shortcut("secondary-shift-c").copy(object.url),
        new Action("Copy Markdown Link").icon("copy").shortcut("secondary-shift-m").copy(`[${title}](${object.url})`),
        new Action("Copy Title").icon("copy").copy(title),
      ]),
      new ActionPanelSection("Write").children([
        create,
        ...(object.object === "page"
          ? [
              new Action("Append to Page…")
                .icon("pencil-line")
                .shortcut("secondary-shift-a")
                .push(() => new AppendForm({ page: object }), "Append to Page"),
            ]
          : []),
      ]),
    ]);
  }

  row(object) {
    const item = new ListItem(object.id, titleOf(object)).icon(itemIcon(object));
    return (object.object === "database" ? item.subtitle("Database") : item)
      .accessory_date(object.last_edited_time)
      .accessory_tooltip("Last edited")
      .detail(this.detail(object))
      .actions(this.actions(object));
  }

  render() {
    const [title, description] = this.error
      ? ["Cannot search Notion", this.error]
      : this.query
        ? ["Nothing found", "Only pages shared with your integration show up: ••• → Connections in Notion."]
        : ["No pages yet", "Share pages with your integration in Notion (••• → Connections) to see them here."];
    return new List()
      .placeholder("Search Notion…")
      .loading(this.loading)
      .showing_detail(this.results.length > 0)
      .empty_title(this.loading ? "Searching…" : title)
      .empty_description(this.loading ? "Asking Notion" : description)
      .on_query_change((query, cx) => this.typed(query, cx))
      .on_selection_change((id, cx) => this.preview(id, cx))
      .children(this.results.map((object) => this.row(object)));
  }
}
