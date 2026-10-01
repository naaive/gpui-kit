// Shared pieces of the Developer Tools commands: where their input comes
// from, the page that shows one result in full, and the row that carries a
// result with Copy and Paste actions.
import { View } from "gpui-kit";
import { Action, ActionPanel, ActionPanelSection, Detail, List, ListItem, MetadataLabel } from "launcher";
import { selected_text } from "launcher/api";

/** The text selected in the application in front, or "" when there is none. */
export function readSelection() {
  try {
    return selected_text() ?? "";
  } catch (_) {
    return "";
  }
}

/** A Markdown code block that holds `text` whatever backticks it contains. */
export function codeBlock(text, language = "") {
  const longest = Math.max(2, ...(String(text).match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(longest + 1);
  return `${fence}${language}\n${text}\n${fence}`;
}

/** Shortens `text` to one line for a subtitle. */
export function preview(text, length = 60) {
  const line = String(text).replace(/\s+/g, " ").trim();
  return line.length > length ? `${line.slice(0, length - 1)}…` : line;
}

/** One result shown in full, with the actions to use it. */
export class ResultView extends View {
  init({ title, value, language = "", metadata = [] }) {
    this.title = title;
    this.value = value;
    this.language = language;
    this.metadata = metadata;
  }

  render() {
    return new Detail(`## ${this.title}\n\n${codeBlock(this.value, this.language)}`)
      .children([
        ...this.metadata.map(([label, text]) => new MetadataLabel(label, text)),
        new MetadataLabel("Length", `${this.value.length} characters`),
      ])
      .actions(
        new ActionPanel().children([
          new Action("Copy to Clipboard").icon("copy").copy(this.value),
          new Action("Paste in Active App").icon("clipboard-paste").paste(this.value),
        ]),
      );
  }
}

/**
 * A row holding a result: Enter copies it, Ctrl/Cmd-Enter pastes it, and
 * Show Result opens it in full. `extra` actions go in a section after those.
 */
export function resultItem(id, title, value, options = {}) {
  const { subtitle, icon = "copy", language = "", accessory, extra = [], keywords = [] } = options;
  const item = keywords.reduce((row, keyword) => row.keyword(keyword), new ListItem(id, title));
  const detail = new Detail(codeBlock(value, language));
  const actions = [
    new Action("Copy to Clipboard").icon("copy").copy(value),
    new Action("Paste in Active App").icon("clipboard-paste").paste(value),
    new Action("Show Result")
      .icon("maximize-2")
      .shortcut("secondary-y")
      .push(() => new ResultView({ title, value, language }), title),
  ];
  const panel = new ActionPanel().children(
    extra.length > 0 ? [...actions, new ActionPanelSection().children(extra)] : actions,
  );
  const row = item
    .icon(icon)
    .subtitle(subtitle ?? preview(value))
    .detail(detail)
    .actions(panel);
  return accessory ? row.accessory(accessory) : row;
}

/**
 * A list whose input is what the user types, or the selected text until they
 * type. Subclasses answer `rows(input)`, and may set `placeholder`,
 * `empty_title` and `empty_description`; `showing_detail` shows each row's
 * result beside the list.
 */
export class TextToolView extends View {
  init() {
    this.selection = readSelection();
    this.query = "";
  }

  get input() {
    return this.query !== "" ? this.query : this.selection;
  }

  /** Whether the input came from the selection rather than the search field. */
  get from_selection() {
    return this.query === "" && this.selection !== "";
  }

  rows(_input) {
    return [];
  }

  page() {
    return { placeholder: "Type or select text…", empty_title: "Type some text", empty_description: "", showing_detail: true };
  }

  render() {
    const page = this.page();
    let rows;
    let failure = null;
    try {
      rows = this.rows(this.input);
    } catch (error) {
      rows = [];
      failure = String(error?.message ?? error);
    }
    return new List()
      .placeholder(page.placeholder)
      .showing_detail(page.showing_detail && rows.length > 0)
      .empty_title(failure ? "Something went wrong" : page.empty_title)
      .empty_description(failure ?? page.empty_description)
      .on_query_change((query, cx) => {
        this.query = query;
        cx.notify();
      })
      .children(rows);
  }
}
