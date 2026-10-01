// Searches the Markdown notes of every Obsidian vault by title, folder and,
// once they are read in the background, content. The selected note's text
// shows beside the list; actions open it in Obsidian or the default app,
// reveal it, or copy a link to it. Opened from "Search Vaults" with a vault
// in its `context`, it searches that vault only.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  Detail,
  List,
  ListItem,
  ListSection,
  MetadataLabel,
  MetadataSeparator,
} from "launcher";
import { launch } from "launcher/api";
import { NO_VAULTS, listNotes, loadVaults, obsidianUri, readNote } from "../lib/vaults.js";

const MAX_SHOWN = 200;
const MAX_INDEXED = 2000;
const INDEXED_CHARS = 20000;
const PREVIEW_CHARS = 6000;

function stripFrontmatter(text) {
  return text.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n?/, "");
}

function preview(note, text) {
  if (text === undefined) return `# ${note.title}\n\n*Loading…*`;
  if (text === null) return `# ${note.title}\n\n*This note cannot be read.*`;
  const body = stripFrontmatter(text).trim();
  const shown = body.length > PREVIEW_CHARS ? `${body.slice(0, PREVIEW_CHARS)}\n\n…` : body;
  return shown.startsWith("# ") ? shown : `# ${note.title}\n\n${shown || "*Empty note*"}`;
}

export default class SearchNotes extends View {
  init(_props, cx) {
    const context = launch().context;
    this.only_vault = context && typeof context === "object" ? context.vault ?? null : null;
    this.search_content = launch().preferences.searchContent !== false;
    this.vaults = [];
    this.notes = [];
    this.texts = new Map();
    this.query = "";
    this.loading = true;
    this.error = null;
    this.load(cx);
  }

  load(cx) {
    cx.spawn(async (task) => {
      try {
        const vaults = (await loadVaults()).filter((vault) => !this.only_vault || vault.path === this.only_vault);
        this.vaults = vaults;
        const notes = [];
        for (const vault of vaults) notes.push(...(await listNotes(vault)));
        this.notes = notes;
        // The first note is selected before any selection change arrives.
        if (notes.length > 0) await this.read(notes[0]);
        this.error = vaults.length === 0 ? NO_VAULTS : null;
      } catch (error) {
        this.error = { title: "Cannot read your vaults", description: String(error?.message ?? error) };
      }
      this.loading = false;
      task.notify();
      if (this.search_content) await this.index(task);
    });
  }

  /** Reads note contents a batch at a time, so content search grows while the list is usable. */
  async index(task) {
    const pending = this.notes.filter((note) => !this.texts.has(note.id)).slice(0, MAX_INDEXED);
    for (const [ix, note] of pending.entries()) {
      await this.read(note);
      if (ix % 100 === 99 && this.query) task.notify();
    }
    if (this.query) task.notify();
  }

  async read(note) {
    if (this.texts.has(note.id)) return this.texts.get(note.id);
    let text = null;
    try {
      text = await readNote(note.path);
    } catch (_) {
      text = null;
    }
    this.texts.set(note.id, text === null ? null : text.slice(0, INDEXED_CHARS));
    return text;
  }

  selected(id, cx) {
    const note = this.notes.find((each) => each.id === id);
    if (!note || this.texts.has(note.id)) return;
    cx.spawn(async (task) => {
      await this.read(note);
      task.notify();
    });
  }

  typed(query, cx) {
    this.query = query.trim();
    cx.notify();
  }

  /** Notes whose title or folder holds every word first, then those whose text does. */
  matches() {
    const words = this.query.toLowerCase().split(/\s+/).filter(Boolean);
    if (words.length === 0) return { titled: this.notes.slice(0, MAX_SHOWN), content: [] };
    const titled = [];
    const content = [];
    for (const note of this.notes) {
      const name = `${note.title} ${note.folder}`.toLowerCase();
      if (words.every((word) => name.includes(word))) {
        titled.push(note);
      } else {
        const text = this.texts.get(note.id);
        if (text && words.every((word) => text.toLowerCase().includes(word))) content.push(note);
      }
      if (titled.length + content.length >= MAX_SHOWN) break;
    }
    return { titled, content };
  }

  row(note) {
    const uri = obsidianUri(note.vault, note.relative);
    const location = note.folder ? `${note.vault.name}/${note.folder}` : note.vault.name;
    return new ListItem(note.id, note.title)
      .icon("file-text")
      .subtitle(location)
      .detail(
        new Detail(preview(note, this.texts.get(note.id))).children([
          new MetadataLabel("Vault", note.vault.name),
          new MetadataLabel("Folder", note.folder || "/"),
          new MetadataSeparator(),
          new MetadataLabel("File", note.relative),
        ]),
      )
      .actions(
        new ActionPanel().children([
          new Action("Open in Obsidian").icon("book-open").open_url(uri),
          new Action("Open in Default App").icon("external-link").open(note.path),
          new Action("Reveal in File Manager").icon("folder-open").shortcut("secondary-shift-r").reveal(note.path),
          new ActionPanelSection("Copy").children([
            new Action("Copy Markdown Link").icon("link").shortcut("secondary-shift-c").copy(`[${note.title}](${uri})`),
            new Action("Copy Obsidian URI").icon("copy").shortcut("secondary-shift-u").copy(uri),
            new Action("Copy Wiki Link").icon("copy").copy(`[[${note.title}]]`),
            new Action("Copy Path").icon("copy").copy(note.path),
          ]),
          new ActionPanelSection("Note").children([
            new Action("Create Note").icon("notebook-pen").shortcut("secondary-n").launch("create-note"),
          ]),
        ]),
      );
  }

  render() {
    const { titled, content } = this.matches();
    const empty = this.error ?? (this.notes.length === 0
      ? { title: "No notes yet", description: "Your vaults hold no Markdown notes." }
      : { title: "No matching notes", description: this.search_content ? "Try another word." : "Try another title." });
    const sections = this.query
      ? [
          ...(titled.length > 0 ? [new ListSection("Titles").children(titled.map((note) => this.row(note)))] : []),
          ...(content.length > 0 ? [new ListSection("Content").children(content.map((note) => this.row(note)))] : []),
        ]
      : titled.map((note) => this.row(note));
    return new List()
      .placeholder(this.only_vault ? "Search notes in this vault…" : "Search notes…")
      .loading(this.loading)
      .showing_detail(titled.length + content.length > 0)
      .empty_title(this.loading ? "Reading your vaults…" : empty.title)
      .empty_description(this.loading ? "" : empty.description)
      .on_query_change((query, cx) => this.typed(query, cx))
      .on_selection_change((id, cx) => this.selected(id, cx))
      .children(sections);
  }
}
