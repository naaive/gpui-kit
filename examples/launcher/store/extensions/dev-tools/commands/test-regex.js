// Tests a regular expression. Type `/pattern/flags text` in the search
// field, or a bare pattern to run it on the selected text, or open the form
// for longer text. Each match is a row, with its groups beside it.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  Detail,
  Form,
  ListItem,
  ListSection,
  MetadataLabel,
  MetadataSeparator,
  TextArea,
  TextField,
} from "launcher";
import { pop } from "launcher/api";
import { codeBlock, preview, TextToolView } from "../lib/ui.js";

const MAX_MATCHES = 500;

/** Splits `/pattern/flags text` into its parts, or answers null. */
export function parseQuery(query) {
  if (!query.startsWith("/")) return null;
  let inClass = false;
  for (let at = 1; at < query.length; at += 1) {
    const char = query[at];
    if (char === "\\") {
      at += 1;
    } else if (char === "[") {
      inClass = true;
    } else if (char === "]") {
      inClass = false;
    } else if (char === "/" && !inClass) {
      const rest = query.slice(at + 1);
      const flags = /^[dgimsuyv]*/.exec(rest)[0];
      return { pattern: query.slice(1, at), flags, text: rest.slice(flags.length).replace(/^ /, "") };
    }
  }
  return null;
}

/** Every match of `pattern` in `text`: `{ index, value, groups, named }`. */
export function findMatches(pattern, flags, text) {
  const global = flags.includes("g");
  // Built as typed first, so an error names the user's own flags.
  new RegExp(pattern, flags);
  const regex = new RegExp(pattern, global ? flags : `${flags}g`);
  const matches = [];
  let match;
  while ((match = regex.exec(text)) !== null && matches.length < MAX_MATCHES) {
    matches.push({ index: match.index, value: match[0], groups: match.slice(1), named: match.groups ?? {} });
    if (match[0] === "") regex.lastIndex += 1;
    if (!global) break;
  }
  return matches;
}

class RegexForm extends View {
  init({ pattern, flags, text, on_done }) {
    this.values = { pattern, flags, text };
    this.on_done = on_done;
  }

  render() {
    return new Form()
      .actions(
        new ActionPanel().child(
          new Action("Test Regex").submit((values) => {
            this.on_done({
              pattern: String(values.pattern ?? ""),
              flags: String(values.flags ?? "").replace(/[^dgimsuyv]/g, ""),
              text: String(values.text ?? ""),
            });
            pop();
          }),
        ),
      )
      .children([
        new TextField("pattern", "Pattern").placeholder("(\\w+)@(\\w+)\\.com").default_value(this.values.pattern),
        new TextField("flags", "Flags").placeholder("gimsuy").default_value(this.values.flags),
        new TextArea("text", "Text").placeholder("Text to search").default_value(this.values.text),
      ]);
  }
}

function matchDetail(match) {
  const groups = match.groups.map((group, index) => new MetadataLabel(`Group ${index + 1}`, group ?? "(no match)"));
  const named = Object.entries(match.named).map(([name, value]) => new MetadataLabel(`<${name}>`, value ?? "(no match)"));
  return new Detail(codeBlock(match.value)).children([
    new MetadataLabel("Index", String(match.index)),
    new MetadataLabel("Length", String(match.value.length)),
    ...(groups.length + named.length > 0 ? [new MetadataSeparator(), ...groups, ...named] : []),
  ]);
}

export default class TestRegex extends TextToolView {
  init() {
    super.init();
    this.form = null;
  }

  /** Pattern, flags and text from the form, the search field, or the selection. */
  parts() {
    if (this.query === "" && this.form) return this.form;
    const parsed = parseQuery(this.query);
    if (parsed) return { ...parsed, text: parsed.text || this.selection };
    return { pattern: this.query, flags: "g", text: this.selection };
  }

  formAction(parts) {
    return new Action("Edit in Form…")
      .icon("pencil")
      .shortcut("secondary-e")
      .push(
        () =>
          new RegexForm({
            ...parts,
            on_done: (form) => {
              this.form = form;
              this.query = "";
            },
          }),
        "Test Regex",
      );
  }

  page() {
    return {
      placeholder: "/pattern/flags text to search…",
      empty_title: "Test a Regular Expression",
      empty_description: "Type /pattern/flags followed by text, or a pattern to run on the selected text.",
      showing_detail: true,
    };
  }

  hint(parts, title, subtitle, icon = "regex", tone = null) {
    const item = new ListItem("hint", title).icon(icon).subtitle(subtitle);
    return [
      (tone ? item.icon_tone(tone) : item)
        .detail(new Detail(`## ${title}\n\n${subtitle}\n\nExample: \`/(\\d+)-(\\d+)/g 10-20 and 30-40\``))
        .actions(new ActionPanel().child(this.formAction(parts))),
    ];
  }

  rows() {
    const parts = this.parts();
    if (parts.pattern === "") return this.hint(parts, "Test a Regular Expression", "Type /pattern/flags and text, or open the form");
    let matches;
    try {
      matches = findMatches(parts.pattern, parts.flags, parts.text);
    } catch (error) {
      return this.hint(parts, "Invalid Pattern", String(error?.message ?? error), "circle-alert", "danger");
    }
    if (parts.text === "") return this.hint(parts, "No Text to Search", "Add text after the pattern, select some first, or open the form");
    const literal = `/${parts.pattern}/${parts.flags}`;
    const all = matches.map((match) => match.value).join("\n");
    const summary = new ListItem("summary", matches.length === 0 ? "No Matches" : `${matches.length} ${matches.length === 1 ? "Match" : "Matches"}`)
      .icon("regex")
      .icon_tone(matches.length === 0 ? "warning" : "success")
      .subtitle(literal)
      .accessory(`in ${parts.text.length} characters`)
      .detail(new Detail(`## ${literal}\n\n${codeBlock(parts.text)}`))
      .actions(
        new ActionPanel().children([
          new Action("Copy All Matches").icon("copy").copy(all),
          new Action("Copy Regex").icon("copy").copy(literal),
          this.formAction(parts),
        ]),
      );
    const rows = matches.map((match, index) => {
      const item = new ListItem(`match-${index}`, match.value === "" ? "(empty match)" : preview(match.value))
        .icon("text-search")
        .accessory(`@${match.index}`)
        .detail(matchDetail(match));
      const groups = match.groups.map((group) => group ?? "∅").join(" · ");
      return (match.groups.length > 0 ? item.subtitle(groups) : item).actions(
        new ActionPanel().children([
          new Action("Copy Match").icon("copy").copy(match.value),
          new Action("Copy All Matches").icon("copy").shortcut("secondary-shift-c").copy(all),
          ...match.groups.map((group, group_index) =>
            new Action(`Copy Group ${group_index + 1}`).copy(group ?? ""),
          ),
          new Action("Copy Regex").copy(literal),
          this.formAction(parts),
        ]),
      );
    });
    return [summary, ...(rows.length > 0 ? [new ListSection("Matches").children(rows)] : [])];
  }
}
