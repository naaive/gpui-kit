// The form both "Create Note" and "Edit Note" show. It is a View of its own,
// so the list can `push` it and a command can default-export a subclass.
import { View } from "gpui-kit";
import { Action, ActionPanel, Form, TextArea, TextField } from "launcher";
import { pop, show_hud } from "launcher/api";
import { putNote } from "./store.js";

export class NoteForm extends View {
  /**
   * `note` is the note to edit, or none to create one titled `title`.
   * `on_saved(note)` runs after saving an edit; the form then returns to the
   * page that pushed it. Without it, saving hides the launcher.
   */
  init({ note = null, title = "", on_saved = null } = {}) {
    this.note = note;
    this.title = note?.title ?? title;
    this.on_saved = on_saved;
    this.error = null;
  }

  save(values, cx) {
    const title = String(values.title ?? "").trim();
    if (!title) {
      this.error = "Give the note a title.";
      cx.notify();
      return;
    }
    const note = putNote({ id: this.note?.id, title, body: String(values.body ?? "") });
    if (this.on_saved) {
      this.on_saved(note);
      pop();
    } else {
      show_hud(`Saved “${title}”`);
    }
  }

  render() {
    const title = new TextField("title", "Title").placeholder("Untitled").default_value(this.title);
    return new Form()
      .actions(
        new ActionPanel().child(
          new Action(this.note ? "Save Changes" : "Create Note").submit((values, cx) => this.save(values, cx)),
        ),
      )
      .children([
        this.error ? title.error(this.error) : title,
        new TextArea("body", "Note").placeholder("Write something…").default_value(this.note?.body ?? ""),
      ]);
  }
}
