// Creates a Todoist task. Typed with text after the command, it adds the
// task at once through Todoist's Quick Add, which understands "tomorrow 9am",
// "#Project", "@label" and "p1" in the text, and confirms with a HUD.
// Without text it shows a form: content, description, project (loaded from
// Todoist), a due date in natural language, and priority.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  Dropdown,
  DropdownItem,
  Form,
  TextArea,
  TextField,
} from "launcher";
import { cache_get, cache_set, launch, open, show_hud, show_toast } from "launcher/api";
import { PRIORITIES, TodoistError, createTask, projects, quickAdd, taskUrl } from "../lib/todoist.js";

const PROJECTS_KEY = "projects";

function describe(error) {
  return error instanceof TodoistError
    ? { title: error.title, message: error.message }
    : { title: "Cannot create the task", message: String(error?.message ?? error) };
}

export default class CreateTask extends View {
  init(_props, cx) {
    const cached = cache_get(PROJECTS_KEY);
    this.projects = Array.isArray(cached) ? cached : [];
    this.saving = false;
    this.content_error = null;
    // Bumped after a save, so the fields start empty again.
    this.round = 0;
    this.content = String(launch().arguments.content ?? "").trim();
    if (this.content) this.quick_add(this.content, cx);
    this.load_projects(cx);
  }

  load_projects(cx) {
    cx.spawn(async (task) => {
      try {
        const all = await projects();
        this.projects = all.sort((a, b) => Number(b.inbox) - Number(a.inbox) || a.order - b.order);
        cache_set(PROJECTS_KEY, this.projects);
      } catch (error) {
        if (!this.content) {
          const { title, message } = describe(error);
          show_toast({ title, message, style: "failure", id: "projects" });
        }
      }
      task.notify();
    });
  }

  quick_add(text, cx) {
    this.saving = true;
    cx.spawn(async (task) => {
      try {
        const created = await quickAdd(text);
        show_hud(`Added “${created.content}”`);
        this.content = "";
      } catch (error) {
        // The form stays, filled in, so nothing typed is lost.
        const { title, message } = describe(error);
        show_toast({ title, message, style: "failure" });
      }
      this.saving = false;
      task.notify();
    });
  }

  field(id) {
    return `${id}-${this.round}`;
  }

  submit(values, cx, then_open) {
    const content = String(values[this.field("content")] ?? "").trim();
    if (!content) {
      this.content_error = "Write what needs doing.";
      cx.notify();
      return;
    }
    if (this.saving) return;
    const fields = { content, priority: Number(values.priority ?? 1) || 1 };
    const description = String(values[this.field("description")] ?? "").trim();
    const due = String(values[this.field("due")] ?? "").trim();
    if (description) fields.description = description;
    if (due) fields.due_string = due;
    if (values.project) fields.project_id = values.project;
    this.content_error = null;
    this.saving = true;
    cx.notify();
    cx.spawn(async (task) => {
      try {
        const created = await createTask(fields);
        this.round += 1;
        this.content = "";
        if (then_open) {
          open(taskUrl(created.id));
        } else {
          show_toast({ title: "Task created", message: created.content, style: "success", primary_action: "Open in Todoist" }).then(
            (answer) => answer === "primary" && open(taskUrl(created.id)),
          );
        }
      } catch (error) {
        const { title, message } = describe(error);
        show_toast({ title, message, style: "failure" });
      }
      this.saving = false;
      task.notify();
    });
  }

  render() {
    const content = new TextField(this.field("content"), "Task").placeholder("Buy milk").default_value(this.content);
    const inbox = this.projects.find((each) => each.inbox) ?? this.projects[0];
    const projects = new Dropdown("project", "Project").children(this.projects.map((each) => new DropdownItem(each.id, each.name)));
    const project = inbox ? projects.default_value(inbox.id) : projects;
    return new Form()
      .loading(this.saving)
      .actions(
        new ActionPanel().children([
          new Action("Create Task").icon("plus").submit((values, cx) => this.submit(values, cx, false)),
          new Action("Create and Open in Todoist").icon("external-link").submit((values, cx) => this.submit(values, cx, true)),
        ]),
      )
      .children([
        this.content_error ? content.error(this.content_error) : content,
        new TextArea(this.field("description"), "Description").placeholder("Details, links…"),
        this.projects.length === 0 ? project.info("Projects load from Todoist; without them the task goes to the Inbox.") : project,
        new TextField(this.field("due"), "Due").placeholder("tomorrow 9am, every friday, Oct 12").info("Natural language, as in Todoist."),
        new Dropdown("priority", "Priority")
          .default_value("1")
          .children(PRIORITIES.map((priority) => new DropdownItem(String(priority.value), priority.title))),
      ]);
  }
}
