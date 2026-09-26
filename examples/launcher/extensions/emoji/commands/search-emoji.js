// A grid in sections, a dropdown that narrows it, and a preference that
// decides which action comes first, and so which one Enter performs.
import { View } from "gpui-kit";
import { Action, ActionPanel, List, ListDropdown, ListDropdownItem, ListItem, ListSection } from "launcher";
import { launch } from "launcher/api";
import { CATEGORIES } from "../emoji.js";

export default class SearchEmoji extends View {
  init() {
    this.category = "all";
    this.copy_first = launch().preferences.primary_action === "copy";
  }

  item(emoji) {
    const paste = new Action("Paste Emoji").icon("clipboard-paste").paste(emoji.character);
    const copy = new Action("Copy Emoji").icon("copy").copy(emoji.character);
    // The name is the id: stable across renders, so the selection stays on
    // the emoji while the grid is narrowed.
    return [emoji.name, ...emoji.keywords]
      .reduce((item, keyword) => item.keyword(keyword), new ListItem(emoji.name, emoji.character))
      .subtitle(emoji.name)
      .actions(
        new ActionPanel().children([
          ...(this.copy_first ? [copy, paste] : [paste, copy]),
          new Action("Copy Name").shortcut("secondary-shift-c").copy(emoji.name),
        ]),
      );
  }

  render() {
    const categories = CATEGORIES.filter(({ id }) => this.category === "all" || id === this.category);
    return new List()
      .grid(8)
      .placeholder("Search emoji…")
      .empty_title("No matching emoji")
      .dropdown(
        new ListDropdown("Category")
          .value(this.category)
          .children([
            new ListDropdownItem("all", "All Categories"),
            ...CATEGORIES.map(({ id, title }) => new ListDropdownItem(id, title)),
          ])
          .on_change((value, cx) => {
            this.category = value;
            cx.notify();
          }),
      )
      .children(
        categories.map(({ title, emoji }) =>
          new ListSection(title).children(emoji.map((each) => this.item(each))),
        ),
      );
  }
}
