// A menu-bar command: its `render` returns a `MenuBarExtra`, which the
// launcher shows as an icon in the system tray (the menu bar on macOS). It
// starts once the user opens it, renders again on its `interval` and
// whenever it calls `cx.notify()`, and "Remove from Tray" turns it off.
//
// A menu entry's action is any `Action`: opening a link, launching another
// command, or `run` calling back into this view.
import { View } from "gpui-kit";
import {
  Action,
  MenuBarExtra,
  MenuBarItem,
  MenuBarSection,
  MenuBarSeparator,
  MenuBarSubmenu,
} from "launcher";
import { launch } from "launcher/api";

const LINKS = [
  { title: "GPUI Kit", url: "https://gpui-kit.com" },
  { title: "Design Guides", url: "https://gpui-kit.com/docs/design-guides" },
  { title: "Coding Guides", url: "https://gpui-kit.com/docs/coding-guides" },
];

export default class TrayLinks extends View {
  init() {
    this.opened = 0;
    this.refreshed = new Date();
    this.reason = launch().launch_type;
  }

  render() {
    const open = (link) =>
      new MenuBarItem(link.title).action(
        new Action(link.title).run((cx) => {
          this.opened += 1;
          cx.notify();
        }),
      );
    return new MenuBarExtra()
      .icon("book-open")
      .title(this.opened > 0 ? `${this.opened}` : "")
      .tooltip("GPUI Kit")
      .children([
        new MenuBarSection("Documentation").children(
          LINKS.map((link) =>
            new MenuBarItem(link.title).action(new Action(link.title).open_url(link.url)),
          ),
        ),
        new MenuBarSubmenu("Count a Click").children(LINKS.map(open)),
        new MenuBarSeparator(),
        new MenuBarItem("Getting Started Checklist").action(
          new Action("Open Checklist").launch("checklist"),
        ),
        new MenuBarItem(`Refreshed ${this.refreshed.toLocaleTimeString()}`).subtitle(this.reason),
      ]);
  }
}
