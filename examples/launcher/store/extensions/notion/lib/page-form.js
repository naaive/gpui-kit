// The Create Page form: a parent page or database, a title and some text,
// which becomes a new page of paragraphs. Quick Note default-exports it;
// Search Notion pushes it with the parent chosen.
import { Action, ActionPanel, Dropdown, Form, TextArea, TextField } from "launcher";
import { FormState } from "launcher/utils";
import { createPage } from "./notion.js";
import { parentItems } from "./present.js";
import { TargetForm } from "./target-form.js";

function blank(parent = null, title = "") {
  return { parent, title, content: "" };
}

export class PageForm extends TargetForm {
  init({ parent = null, title = "" } = {}, cx) {
    this.form = new FormState(blank(null, title), {
      parent: FormState.required("Choose where the page goes"),
      title: FormState.required("Give the page a title"),
    });
    super.init({ target: parent, remember_key: "last-parent" }, cx);
  }

  chooseInitial(id) {
    if (!this.form.value("parent")) this.form.set("parent", id);
  }

  afterSave() {
    this.form.reset(blank(this.form.value("parent")));
  }

  submit(values, cx) {
    if (!this.form.validate(values)) {
      cx.notify();
      return;
    }
    const target = this.targetById(this.form.value("parent"));
    if (!target) return;
    const title = String(this.form.value("title")).trim();
    const content = String(this.form.value("content") ?? "");
    this.save(
      cx,
      "Creating page…",
      async () => ({ page: await createPage(target, title, content), target }),
      "Page created",
    );
  }

  parentField() {
    const field = new Dropdown("parent", "Parent").children(parentItems(this.targets));
    // A dropdown's value must be one of its items: bind it once they are in.
    return this.targetById(this.form.value("parent")) ? this.form.bind("parent", field) : field;
  }

  render() {
    return new Form()
      .loading(this.loading || this.saving)
      .actions(
        new ActionPanel().child(
          new Action("Create Page").icon("file-plus").submit((values, cx) => this.submit(values, cx)),
        ),
      )
      .children([
        ...this.errorNotice(),
        this.parentField().info("Pages and databases shared with your integration"),
        this.form.bind("title", new TextField("title", "Title").placeholder("Untitled")),
        this.form.bind("content", new TextArea("content", "Content").placeholder("Each line becomes a paragraph")),
      ]);
  }
}
