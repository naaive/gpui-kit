// A list with a detail pane, actions grouped into sections, a confirmed
// destructive action, and a pushed form that reports back to the list.
import { View } from "gpui-kit";
import { Action, ActionPanel, ActionPanelSection, Detail, List, ListItem, MetadataLabel } from "launcher";
import { show_toast, update_command_metadata } from "launcher/api";
import { NoteForm } from "../lib/note-form.js";
import { deleteNote, loadNotes } from "../lib/store.js";

function formatDate(time) {
  return new Date(time).toISOString().slice(0, 10);
}

export default class SearchNotes extends View {
  init(props, cx) {
    // This `cx` outlives `init`, so a pushed form can re-render the list
    // after it saves.
    this.cx = cx;
    this.reload();
  }

  reload() {
    this.notes = loadNotes();
    const count = this.notes.length;
    update_command_metadata({ subtitle: count === 0 ? null : `${count} ${count === 1 ? "note" : "notes"}` });
  }

  remove(note, cx) {
    deleteNote(note.id);
    this.reload();
    show_toast({ title: "Note deleted", message: note.title, style: "success" });
    cx.notify();
  }

  edit(note) {
    return new NoteForm({
      note,
      on_saved: () => {
        this.reload();
        this.cx.notify();
      },
    });
  }

  row(note) {
    // A note may have a title and no body; pasting or copying it then gives
    // the title, since an action needs something to carry.
    const text = note.body.trim() ? note.body : note.title;
    const item = new ListItem(note.id, note.title).icon("sticky-note").accessory(formatDate(note.updated));
    // The body is searched too, without being shown in the row.
    return (note.body.trim() ? item.keyword(note.body) : item)
      .detail(
        new Detail(`# ${note.title}\n\n${note.body}`).children([
          new MetadataLabel("Updated", formatDate(note.updated)),
          new MetadataLabel("Length", `${note.body.length} characters`),
        ]),
      )
      .actions(
        new ActionPanel().children([
          new Action("Paste Note").icon("clipboard-paste").paste(text),
          new Action("Copy Note").icon("copy").copy(text),
          new ActionPanelSection("Manage").children([
            new Action("Edit Note").icon("pencil").shortcut("secondary-e").push(() => this.edit(note)),
            new Action("Create Note").icon("plus").shortcut("secondary-n").launch("create-note"),
            new Action("Delete Note")
              .icon("trash")
              .shortcut("ctrl-x")
              .destructive()
              .confirm("Delete this note?", `“${note.title}” cannot be restored.`)
              .run((cx) => this.remove(note, cx)),
          ]),
        ]),
      );
  }

  render() {
    return new List()
      .placeholder("Search notes…")
      .showing_detail(this.notes.length > 0)
      .empty_title(this.notes.length === 0 ? "No notes yet" : "No matching note")
      .empty_description(
        this.notes.length === 0 ? "Run “Create Note” to write the first one." : "Try another word.",
      )
      .children(this.notes.map((note) => this.row(note)));
  }
}
