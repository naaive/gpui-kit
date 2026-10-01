// Generates random (version 4) UUIDs. Typing a number sets how many; the
// dropdown sets the format, and is remembered.
import { View } from "gpui-kit";
import { randomUUID } from "crypto";
import { Action, ActionPanel, List, ListDropdown, ListDropdownItem, ListItem } from "launcher";

const FORMATS = [
  ["lower", "Lowercase"],
  ["upper", "Uppercase"],
  ["compact", "No Hyphens"],
  ["braces", "With Braces"],
];
const DEFAULT_COUNT = 5;
const MAX_COUNT = 100;
const FORMAT_KEY = "uuid-format";

function formatted(uuid, format) {
  if (format === "upper") return uuid.toUpperCase();
  if (format === "compact") return uuid.replace(/-/g, "");
  if (format === "braces") return `{${uuid.toUpperCase()}}`;
  return uuid;
}

function loadFormat() {
  try {
    const format = localStorage.getItem(FORMAT_KEY);
    return FORMATS.some(([value]) => value === format) ? format : "lower";
  } catch (_) {
    return "lower";
  }
}

export default class GenerateUuid extends View {
  init() {
    this.count = DEFAULT_COUNT;
    this.format = loadFormat();
    this.generate();
  }

  generate() {
    this.uuids = Array.from({ length: this.count }, () => randomUUID());
  }

  typed(query, cx) {
    const count = parseInt(query, 10);
    const next = Number.isFinite(count) && count > 0 ? Math.min(count, MAX_COUNT) : DEFAULT_COUNT;
    if (next !== this.count) {
      this.count = next;
      this.generate();
      cx.notify();
    }
  }

  row(uuid, index, all) {
    const value = formatted(uuid, this.format);
    return new ListItem(`uuid-${index}`, value)
      .icon("fingerprint-pattern")
      .accessory(`#${index + 1}`)
      .actions(
        new ActionPanel().children([
          new Action("Copy UUID").icon("copy").copy(value),
          new Action("Paste UUID").icon("clipboard-paste").paste(value),
          new Action(`Copy All ${all.length} UUIDs`).icon("copy").shortcut("secondary-shift-c").copy(all.join("\n")),
          new Action("Paste All").icon("clipboard-paste").shortcut("secondary-shift-v").paste(all.join("\n")),
          new Action("Generate New UUIDs")
            .icon("refresh-cw")
            .shortcut("secondary-r")
            .run((cx) => {
              this.generate();
              cx.notify();
            }),
        ]),
      );
  }

  render() {
    const all = this.uuids.map((uuid) => formatted(uuid, this.format));
    return new List()
      .placeholder(`How many? (${DEFAULT_COUNT} by default, up to ${MAX_COUNT})`)
      .empty_title("No UUIDs")
      .on_query_change((query, cx) => this.typed(query, cx))
      .dropdown(
        new ListDropdown("Format")
          .value(this.format)
          .children(FORMATS.map(([value, title]) => new ListDropdownItem(value, title)))
          .on_change((value, cx) => {
            this.format = value;
            try {
              localStorage.setItem(FORMAT_KEY, value);
            } catch (_) {
              // Not remembering the format is harmless.
            }
            cx.notify();
          }),
      )
      .children(this.uuids.map((uuid, index) => this.row(uuid, index, all)));
  }
}
