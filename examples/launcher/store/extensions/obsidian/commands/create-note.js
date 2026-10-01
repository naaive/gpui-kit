// A form that writes a new Markdown note into a vault: pick the vault, a
// folder inside it, a title and the text. It never overwrites a note; a title
// already taken gets a number. After saving it offers to open the note in
// Obsidian, and the form empties for the next one.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  Dropdown,
  DropdownItem,
  Form,
  FormDescription,
  TextArea,
  TextField,
} from "launcher";
import { close_main_window, launch, open, show_toast } from "launcher/api";
import { NO_VAULTS, createNote, loadVaults, obsidianUri, pickVault } from "../lib/vaults.js";

export default class CreateNote extends View {
  init(_props, cx) {
    this.title = launch().arguments.title ?? "";
    this.vaults = [];
    this.loading = true;
    this.saving = false;
    this.error = null;
    this.title_error = null;
    // Bumped after a save, so the fields start empty again.
    this.round = 0;
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

  field(id) {
    return `${id}-${this.round}`;
  }

  save(values, cx, then_open) {
    const title = String(values[this.field("title")] ?? "").trim();
    const vault = this.vaults.find((each) => each.path === values.vault) ?? pickVault(this.vaults, "");
    if (!title) {
      this.title_error = "Give the note a title.";
      cx.notify();
      return;
    }
    if (!vault || this.saving) return;
    this.title_error = null;
    this.saving = true;
    cx.notify();
    const folder = String(values[this.field("folder")] ?? "");
    const content = String(values[this.field("content")] ?? "");
    cx.spawn(async (task) => {
      try {
        const note = await createNote(vault, folder, title, content);
        const uri = obsidianUri(vault, note.relative);
        if (then_open) {
          open(uri);
          close_main_window();
        } else {
          this.round += 1;
          this.title = "";
          show_toast({ title: "Note created", message: note.relative, style: "success", primary_action: "Open in Obsidian" }).then(
            (answer) => answer === "primary" && open(uri),
          );
        }
      } catch (error) {
        show_toast({ title: "Cannot create the note", message: String(error?.message ?? error), style: "failure" });
      }
      this.saving = false;
      task.notify();
    });
  }

  render() {
    if (!this.loading && this.vaults.length === 0) {
      return new Form().children([
        new FormDescription(this.error ? "Cannot read your vaults" : NO_VAULTS.title, this.error ?? NO_VAULTS.description),
      ]);
    }
    const title = new TextField(this.field("title"), "Title").placeholder("Meeting notes").default_value(this.title);
    const fallback = pickVault(this.vaults, launch().preferences.defaultVault);
    const vaults = new Dropdown("vault", "Vault").children(this.vaults.map((vault) => new DropdownItem(vault.path, vault.name)));
    return new Form()
      .loading(this.loading || this.saving)
      .actions(
        new ActionPanel().children([
          new Action("Create Note").icon("notebook-pen").submit((values, cx) => this.save(values, cx, false)),
          new Action("Create and Open in Obsidian").icon("book-open").submit((values, cx) => this.save(values, cx, true)),
        ]),
      )
      .children([
        fallback ? vaults.default_value(fallback.path) : vaults,
        new TextField(this.field("folder"), "Folder").placeholder("Inbox").info("A folder inside the vault; it is created if missing. Leave empty for the vault's top level."),
        this.title_error ? title.error(this.title_error) : title,
        new TextArea(this.field("content"), "Content").placeholder("Write in Markdown…"),
      ]);
  }
}
