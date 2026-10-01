// Formats JSON: the typed text, the text selected when the launcher opened,
// or what was written in the editor form. Valid JSON gets pretty-printed,
// minified and key-sorted rows; invalid JSON one row saying where it breaks.
import { View } from "gpui-kit";
import { Action, ActionPanel, Detail, Form, FormDescription, ListItem, TextArea } from "launcher";
import { launch, pop } from "launcher/api";
import { parseJson, sortKeys } from "../lib/json.js";
import { codeBlock, resultItem, TextToolView } from "../lib/ui.js";

/** A form for JSON longer than the search field holds comfortably. */
class JsonEditor extends View {
  init({ text, on_done }) {
    this.text = text;
    this.on_done = on_done;
  }

  render() {
    return new Form()
      .actions(
        new ActionPanel().child(
          new Action("Format JSON").submit((values) => {
            this.on_done(String(values.json ?? ""));
            pop();
          }),
        ),
      )
      .children([
        new TextArea("json", "JSON").placeholder('{ "key": "value" }').default_value(this.text),
        new FormDescription("The formatted results replace what the search field shows until you type again."),
      ]);
  }
}

function indentOf(preference) {
  if (preference === "tab") return "\t";
  return Number(preference) || 2;
}

function summary(value) {
  if (Array.isArray(value)) return `Array · ${value.length} ${value.length === 1 ? "item" : "items"}`;
  if (value && typeof value === "object") {
    const count = Object.keys(value).length;
    return `Object · ${count} ${count === 1 ? "key" : "keys"}`;
  }
  return value === null ? "Null" : typeof value === "string" ? "String" : typeof value === "number" ? "Number" : "Boolean";
}

/** The input with a caret under the column where it breaks. */
function errorMarkdown(text, error) {
  const lines = text.split("\n");
  const first = Math.max(0, error.line - 3);
  const shown = lines.slice(first, error.line);
  const caret = `${" ".repeat(Math.max(0, error.column - 1))}^ ${error.message}`;
  return `## Invalid JSON\n\nLine ${error.line}, column ${error.column}: ${error.message}\n\n${codeBlock(
    [...shown, caret].join("\n"),
  )}`;
}

export default class FormatJson extends TextToolView {
  init() {
    super.init();
    this.edited = "";
    this.indent = indentOf(launch().preferences.indent);
  }

  get input() {
    return this.query !== "" ? this.query : this.edited || this.selection;
  }

  editAction() {
    return new Action("Edit Input…")
      .icon("pencil")
      .shortcut("secondary-e")
      .push(
        () =>
          new JsonEditor({
            text: this.input,
            on_done: (text) => {
              this.edited = text;
              this.query = "";
            },
          }),
        "Edit JSON",
      );
  }

  page() {
    return {
      placeholder: "Type or paste JSON…",
      empty_title: "Format JSON",
      empty_description: "Type JSON, or select some before opening the launcher.",
      showing_detail: true,
    };
  }

  rows(input) {
    const extra = [this.editAction()];
    if (input.trim() === "") {
      return [
        new ListItem("editor", "Open JSON Editor")
          .icon("braces")
          .subtitle("Write or paste longer JSON")
          .detail(new Detail("Type JSON in the search field, select some before opening the launcher, or open the editor."))
          .actions(new ActionPanel().child(extra[0])),
      ];
    }
    const parsed = parseJson(input);
    if (parsed.error) {
      const { line, column, message } = parsed.error;
      const report = `Line ${line}, column ${column}: ${message}`;
      return [
        new ListItem("error", "Invalid JSON")
          .icon("circle-alert")
          .icon_tone("danger")
          .subtitle(message)
          .accessory(`Ln ${line}, Col ${column}`)
          .detail(new Detail(errorMarkdown(input, parsed.error)))
          .actions(new ActionPanel().children([this.editAction(), new Action("Copy Error").icon("copy").copy(report)])),
      ];
    }
    const { value } = parsed;
    const kind = summary(value);
    return [
      resultItem("pretty", "Pretty-Printed", JSON.stringify(value, null, this.indent), {
        icon: "braces",
        language: "json",
        accessory: kind,
        extra,
      }),
      resultItem("minified", "Minified", JSON.stringify(value), { icon: "minimize-2", language: "json", extra }),
      resultItem("sorted", "Sorted Keys", JSON.stringify(sortKeys(value), null, this.indent), {
        icon: "arrow-down-a-z",
        language: "json",
        extra,
      }),
      resultItem("string", "Escaped as a String", JSON.stringify(JSON.stringify(value)), {
        icon: "quote",
        extra,
      }),
    ];
  }
}
