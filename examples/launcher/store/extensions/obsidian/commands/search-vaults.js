// Lists the Obsidian vaults, the open ones first: open one in Obsidian,
// search the notes of one, or show its folder.
import { View } from "gpui-kit";
import { Action, ActionPanel, ActionPanelSection, List, ListItem } from "launcher";
import { launch_command } from "launcher/api";
import { NO_VAULTS, loadVaults, obsidianUri } from "../lib/vaults.js";

export default class SearchVaults extends View {
  init(_props, cx) {
    this.vaults = [];
    this.loading = true;
    this.error = null;
    cx.spawn(async (task) => {
      try {
        this.vaults = await loadVaults();
      } catch (error) {
        this.error = String(error?.message ?? error);
      }
      this.loading = false;
      task.notify();
    });
  }

  row(vault) {
    const item = new ListItem(vault.id, vault.name).icon("vault").subtitle(vault.path).keyword(vault.path);
    return (vault.open ? item.tag("Open", "accent") : item)
      .actions(
        new ActionPanel().children([
          new Action("Open in Obsidian").icon("book-open").open_url(obsidianUri(vault)),
          new Action("Search Notes in Vault")
            .icon("search")
            .run(() => launch_command("search-notes", {}, { vault: vault.path })),
          new ActionPanelSection("Folder").children([
            new Action("Open Folder").icon("folder-open").open(vault.path),
            new Action("Reveal in File Manager").icon("folder").shortcut("secondary-shift-r").reveal(vault.path),
            new Action("Copy Path").icon("copy").shortcut("secondary-shift-c").copy(vault.path),
          ]),
        ]),
      );
  }

  render() {
    return new List()
      .placeholder("Search vaults…")
      .loading(this.loading)
      .empty_title(this.loading ? "Reading Obsidian's settings…" : this.error ? "Cannot read your vaults" : NO_VAULTS.title)
      .empty_description(this.loading ? "" : this.error ?? NO_VAULTS.description)
      .children(this.vaults.map((vault) => this.row(vault)));
  }
}
