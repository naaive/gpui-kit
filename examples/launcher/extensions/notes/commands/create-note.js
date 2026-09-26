// A command with an optional argument: typing "Create Note" and a title in
// the root search opens the form with the title filled in.
import { launch } from "launcher/api";
import { NoteForm } from "../lib/note-form.js";

export default class CreateNote extends NoteForm {
  init() {
    super.init({ title: launch().arguments.title ?? "" });
  }
}
