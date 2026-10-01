// The Append to Page form: a page and some text, added at the end of the
// page as paragraphs, or as to-dos. Append to Page default-exports it;
// Search Notion pushes it with the page chosen.
import { Action, ActionPanel, Checkbox, Dropdown, Form, TextArea } from "launcher";
import { FormState } from "launcher/utils";
import { appendBlocks, paragraphs } from "./notion.js";
import { parentItems } from "./present.js";
import { TargetForm } from "./target-form.js";

function blank(page = null) {
  return { page, text: "", todo: false };
}

/** Paragraph blocks, or the same lines as unchecked to-dos. */
function blocksFor(text, todo) {
  const blocks = paragraphs(text);
  if (!todo) return blocks;
  return blocks
    .filter((block) => block.paragraph.rich_text.length > 0)
    .map((block) => ({ object: "block", type: "to_do", to_do: { rich_text: block.paragraph.rich_text, checked: false } }));
}

export class AppendForm extends TargetForm {
  init({ page = null } = {}, cx) {
    this.form = new FormState(blank(), {
      page: FormState.required("Choose a page"),
      text: FormState.required("Write something to add"),
    });
    super.init({ target: page, object: "page", remember_key: "last-append-page" }, cx);
  }

  chooseInitial(id) {
    if (!this.form.value("page")) this.form.set("page", id);
  }

  afterSave() {
    this.form.reset(blank(this.form.value("page")));
  }

  submit(values, cx) {
    if (!this.form.validate(values)) {
      cx.notify();
      return;
    }
    const target = this.targetById(this.form.value("page"));
    if (!target) return;
    const blocks = blocksFor(this.form.value("text"), Boolean(this.form.value("todo")));
    this.save(
      cx,
      "Appending…",
      async () => {
        await appendBlocks(target.id, blocks);
        return { page: target, target };
      },
      "Added to the page",
    );
  }

  pageField() {
    const field = new Dropdown("page", "Page").children(parentItems(this.targets));
    return this.targetById(this.form.value("page")) ? this.form.bind("page", field) : field;
  }

  render() {
    return new Form()
      .loading(this.loading || this.saving)
      .actions(
        new ActionPanel().child(
          new Action("Append to Page").icon("pencil-line").submit((values, cx) => this.submit(values, cx)),
        ),
      )
      .children([
        ...this.errorNotice(),
        this.pageField(),
        this.form.bind("text", new TextArea("text", "Text").placeholder("Each line becomes a paragraph")),
        this.form.bind("todo", new Checkbox("todo", "Format", "Add each line as a to-do")),
      ]);
  }
}
