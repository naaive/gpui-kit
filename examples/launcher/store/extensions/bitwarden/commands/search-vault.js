// Searches the Bitwarden vault through the `bw` CLI. A locked vault asks for
// the master password first; the session key `bw unlock` answers lives in a
// field of this page only, and is gone when the page closes. Secrets are
// never shown: rows and the detail carry names, usernames and addresses, and
// a password, TOTP or card number is asked of `bw` only when it is copied or
// pasted. Secrets are copied concealed: Clipboard History leaves them out and
// the launcher clears them from the clipboard after 30 seconds.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  Detail,
  Form,
  FormDescription,
  List,
  ListItem,
  ListSection,
  MetadataLabel,
  MetadataLink,
  MetadataSeparator,
  MetadataTags,
  PasswordField,
} from "launcher";
import { copy, paste, show_hud, show_toast } from "launcher/api";
import {
  BwError,
  ITEM_TYPES,
  getCardField,
  getField,
  hostOf,
  listFolders,
  listItems,
  loginHint,
  openableUrl,
  status,
  sync,
  unlock,
} from "../lib/bw.js";


export default class SearchVault extends View {
  init(_props, cx) {
    // "checking" | "locked" | "unlocking" | "loading" | "ready" | "problem"
    this.phase = "checking";
    this.problem = null;
    this.password_error = null;
    this.session = null;
    this.email = null;
    this.items = [];
    this.folders = new Map();
    this.check(cx);
  }

  fail(error) {
    this.phase = "problem";
    this.problem = error instanceof BwError
      ? { title: error.title, description: error.hint }
      : { title: "Something went wrong", description: String(error?.message ?? error) };
  }

  check(cx) {
    cx.spawn(async (task) => {
      try {
        const state = await status();
        this.email = state.userEmail ?? null;
        if (state.status === "unauthenticated") {
          this.phase = "problem";
          this.problem = { title: "Not logged in to Bitwarden", description: loginHint() };
        } else {
          this.phase = "locked";
        }
      } catch (error) {
        this.fail(error);
      }
      task.notify();
    });
  }

  unlock(values, cx) {
    const password = String(values.password ?? "");
    if (!password) {
      this.password_error = "Enter your master password.";
      cx.notify();
      return;
    }
    this.phase = "unlocking";
    this.password_error = null;
    cx.notify();
    cx.spawn(async (task) => {
      try {
        this.session = await unlock(password);
        await this.load();
      } catch (error) {
        if (error instanceof BwError && error.title === "Invalid master password") {
          this.phase = "locked";
          this.password_error = "That is not your master password.";
        } else {
          this.fail(error);
        }
      }
      task.notify();
    });
  }

  async load() {
    this.phase = "loading";
    const [items, folders] = await Promise.all([listItems(this.session), listFolders(this.session).catch(() => [])]);
    this.items = items;
    this.folders = new Map(folders.filter((folder) => folder.id).map((folder) => [folder.id, folder.name]));
    this.phase = "ready";
  }

  resync(cx) {
    show_toast({ title: "Syncing vault…", style: "progress", id: "sync" });
    cx.spawn(async (task) => {
      try {
        await sync(this.session);
        await this.load();
        show_toast({ title: "Vault synced", style: "success", id: "sync" });
      } catch (error) {
        this.report(error);
      }
      task.notify();
    });
  }

  report(error) {
    const title = error instanceof BwError ? error.title : "Bitwarden CLI failed";
    const message = error instanceof BwError ? error.hint : String(error?.message ?? error);
    show_toast({ title, message, style: "failure", id: "sync" });
  }

  /** Asks `bw` for a secret, then copies or pastes it; a copy is cleared after 30 seconds. */
  secret(fetch_secret, label, how, cx) {
    cx.spawn(async (task) => {
      let value;
      try {
        value = await fetch_secret();
      } catch (error) {
        this.report(error);
        return;
      }
      if (!value) {
        show_toast({ title: `No ${label.toLowerCase()} to ${how}`, style: "failure" });
        return;
      }
      if (how === "paste") {
        paste(value, { concealed: true });
        return;
      }
      copy(value, { concealed: true });
      show_hud(`Copied ${label}, clears in 30 seconds`);
    });
  }

  copy_action(title, label, fetch_secret, shortcut) {
    const action = new Action(title).icon("copy").run((cx) => this.secret(fetch_secret, label, "copy", cx));
    return shortcut ? action.shortcut(shortcut) : action;
  }

  actions(item) {
    const session = this.session;
    const primary = [];
    if (item.type === 1) {
      if (item.has_password) primary.push(this.copy_action("Copy Password", "Password", () => getField("password", item.id, session)));
      if (item.username) primary.push(new Action("Copy Username").icon("user").shortcut("secondary-shift-c").copy(item.username));
      if (item.has_totp) primary.push(this.copy_action("Copy TOTP", "TOTP", () => getField("totp", item.id, session), "secondary-shift-t"));
      if (item.has_password) {
        primary.push(
          new Action("Paste Password")
            .icon("clipboard-paste")
            .shortcut("secondary-shift-v")
            .run((cx) => this.secret(() => getField("password", item.id, session), "Password", "paste", cx)),
        );
      }
      if (item.uris.length > 0) primary.push(new Action("Open URL").icon("external-link").shortcut("secondary-o").open_url(openableUrl(item.uris[0])));
    } else if (item.type === 3) {
      if (item.has_card_number) primary.push(this.copy_action("Copy Card Number", "Card Number", () => getCardField("number", item.id, session)));
      if (item.has_card_code) primary.push(this.copy_action("Copy Security Code", "Security Code", () => getCardField("code", item.id, session)));
    }
    if (item.has_notes) primary.push(this.copy_action("Copy Notes", "Notes", () => getField("notes", item.id, session)));
    return new ActionPanel().children([
      ...primary,
      new ActionPanelSection("Vault").children([
        new Action("Sync Vault").icon("refresh-cw").shortcut("secondary-r").run((cx) => this.resync(cx)),
        new Action("Lock Vault").icon("lock").shortcut("secondary-shift-l").launch("lock-vault"),
      ]),
    ]);
  }

  detail(item) {
    const type = ITEM_TYPES[item.type] ?? { title: "Item" };
    const fields = [new MetadataLabel("Type", type.title)];
    if (item.username) fields.push(new MetadataLabel("Username", item.username));
    if (item.type === 1) fields.push(new MetadataLabel("Password", item.has_password ? "Saved" : "None"));
    if (item.has_totp) fields.push(new MetadataLabel("TOTP", "Set up"));
    if (item.card_brand) fields.push(new MetadataLabel("Brand", item.card_brand));
    if (item.card_expiry) fields.push(new MetadataLabel("Expires", item.card_expiry));
    if (item.identity_name) fields.push(new MetadataLabel("Name", item.identity_name));
    const folder = item.folder_id ? this.folders.get(item.folder_id) : null;
    if (folder) fields.push(new MetadataLabel("Folder", folder));
    if (item.favorite || item.organization) {
      let tags = new MetadataTags("Tags");
      if (item.favorite) tags = tags.tag("Favorite", "warning");
      if (item.organization) tags = tags.tag("Organization", "accent");
      fields.push(tags);
    }
    if (item.uris.length > 0) {
      fields.push(new MetadataSeparator());
      item.uris.slice(0, 5).forEach((uri, ix) => fields.push(new MetadataLink(ix === 0 ? "Website" : `Website ${ix + 1}`, hostOf(uri), openableUrl(uri))));
    }
    // The body names the item and nothing secret.
    return new Detail(`# ${item.name.replace(/[#*_`[\]]/g, "")}\n\n${type.title}`).children(fields);
  }

  row(item) {
    const type = ITEM_TYPES[item.type] ?? { title: "Item", icon: "shield" };
    const subtitle = item.username ?? (item.card_brand ? `${item.card_brand}` : item.identity_name ?? "");
    let row = new ListItem(item.id, item.name).icon(type.icon).subtitle(subtitle).keyword(type.title);
    for (const uri of item.uris.slice(0, 3)) row = row.keyword(hostOf(uri));
    if (item.favorite) row = row.accessory_icon("star").accessory_tooltip("Favorite");
    return row.detail(this.detail(item)).actions(this.actions(item));
  }

  unlock_form() {
    const field = new PasswordField("password", "Master Password").placeholder("Master password");
    return new Form()
      .loading(this.phase === "unlocking")
      .actions(new ActionPanel().child(new Action("Unlock Vault").icon("lock-keyhole").submit((values, cx) => this.unlock(values, cx))))
      .children([
        new FormDescription("Vault Locked", this.email ? `Unlock the vault of ${this.email}. The session lasts while this page is open.` : "Unlock your vault. The session lasts while this page is open."),
        this.password_error ? field.error(this.password_error) : field,
      ]);
  }

  render() {
    if (this.phase === "locked" || this.phase === "unlocking") return this.unlock_form();
    const busy = this.phase === "checking" || this.phase === "loading";
    const favorites = this.items.filter((item) => item.favorite);
    const others = this.items.filter((item) => !item.favorite);
    const empty = this.phase === "problem"
      ? this.problem
      : busy
        ? { title: this.phase === "checking" ? "Checking Bitwarden…" : "Loading your vault…", description: "" }
        : { title: "No matching items", description: "Try a name, username or website." };
    return new List()
      .placeholder("Search vault…")
      .loading(busy)
      .showing_detail(this.items.length > 0)
      .empty_title(empty.title)
      .empty_description(empty.description)
      .children([
        ...(favorites.length > 0 ? [new ListSection("Favorites").children(favorites.map((item) => this.row(item)))] : []),
        new ListSection("Items").children(others.map((item) => this.row(item))),
      ]);
  }
}
