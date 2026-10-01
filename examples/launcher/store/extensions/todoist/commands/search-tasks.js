// Lists the active Todoist tasks: overdue and today's first, then upcoming
// days, then those without a date; or grouped by project, chosen beside the
// search field. The last list shows at once from the cache while a fresh one
// loads. Completing a task removes it at once and offers Undo; priority,
// due date and deletion are changed from the action panel.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  ActionPanelSubmenu,
  List,
  ListDropdown,
  ListDropdownItem,
  ListItem,
  ListSection,
} from "launcher";
import { cache_get, cache_set, show_toast, update_command_metadata } from "launcher/api";
import {
  PRIORITIES,
  TodoistError,
  activeTasks,
  closeTask,
  compareTasks,
  deleteTask,
  dueBucket,
  dueText,
  isoDay,
  priorityOf,
  projectUrl,
  projects,
  reopenTask,
  taskUrl,
  updateTask,
} from "../lib/todoist.js";

const CACHE_KEY = "search-tasks";
const GROUP_KEY = "search-tasks-group";

const RESCHEDULE = [
  { title: "Today", due_string: "today", icon: "sun" },
  { title: "Tomorrow", due_string: "tomorrow", icon: "calendar" },
  { title: "Next Week", due_string: "next monday", icon: "calendar-days" },
];

function problem(error) {
  return error instanceof TodoistError
    ? { title: error.title, description: error.message }
    : { title: "Cannot load tasks", description: String(error?.message ?? error) };
}

export default class SearchTasks extends View {
  init(_props, cx) {
    const cached = cache_get(CACHE_KEY);
    this.tasks = Array.isArray(cached?.tasks) ? cached.tasks : [];
    this.projects = Array.isArray(cached?.projects) ? cached.projects : [];
    this.group = localStorage.getItem(GROUP_KEY) === "project" ? "project" : "date";
    this.loading = true;
    this.error = null;
    this.refresh(cx);
  }

  refresh(cx) {
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        const [tasks, all_projects] = await Promise.all([activeTasks(), projects()]);
        this.tasks = tasks.sort(compareTasks);
        this.projects = all_projects.sort((a, b) => Number(b.inbox) - Number(a.inbox) || a.order - b.order);
        this.error = null;
        this.remember();
      } catch (error) {
        this.error = problem(error);
        if (this.tasks.length > 0) show_toast({ title: this.error.title, message: this.error.description, style: "failure" });
      }
      this.loading = false;
      task.notify();
    });
  }

  /** Caches the list, which holds no secret, and shows today's count in the root search. */
  remember() {
    try {
      cache_set(CACHE_KEY, { tasks: this.tasks, projects: this.projects });
    } catch (_) {
      // The cache is a convenience.
    }
    const due = this.tasks.filter((task) => ["overdue", "today"].includes(dueBucket(task))).length;
    update_command_metadata({ subtitle: due === 0 ? null : `${due} due today` });
  }

  project_name(id) {
    return this.projects.find((project) => project.id === id)?.name ?? "";
  }

  replace(updated) {
    this.tasks = this.tasks.map((task) => (task.id === updated.id ? updated : task)).sort(compareTasks);
  }

  complete(task, cx) {
    this.tasks = this.tasks.filter((each) => each.id !== task.id);
    cx.notify();
    cx.spawn(async (async_cx) => {
      try {
        await closeTask(task.id);
      } catch (error) {
        this.tasks = [...this.tasks, task].sort(compareTasks);
        const { title, description } = problem(error);
        show_toast({ title, message: description, style: "failure" });
        async_cx.notify();
        return;
      }
      // A recurring task moves to its next date instead of going away.
      if (task.due?.recurring) this.refresh(async_cx);
      this.remember();
      const answer = await show_toast({ title: "Task completed", message: task.content, style: "success", primary_action: "Undo" });
      if (answer !== "primary") return;
      try {
        await reopenTask(task.id);
        if (task.due?.recurring) {
          this.refresh(async_cx);
        } else {
          this.tasks = [...this.tasks.filter((each) => each.id !== task.id), task].sort(compareTasks);
          this.remember();
        }
        show_toast({ title: "Task reopened", message: task.content, style: "success" });
      } catch (error) {
        const { title, description } = problem(error);
        show_toast({ title, message: description, style: "failure" });
      }
      async_cx.notify();
    });
  }

  change(task, changes, optimistic, done, cx) {
    const before = task;
    this.replace({ ...task, ...optimistic });
    cx.notify();
    cx.spawn(async (async_cx) => {
      try {
        this.replace(await updateTask(task.id, changes));
        this.remember();
        show_toast({ title: done, message: task.content, style: "success" });
      } catch (error) {
        this.replace(before);
        const { title, description } = problem(error);
        show_toast({ title, message: description, style: "failure" });
      }
      async_cx.notify();
    });
  }

  remove(task, cx) {
    this.tasks = this.tasks.filter((each) => each.id !== task.id);
    cx.notify();
    cx.spawn(async (async_cx) => {
      try {
        await deleteTask(task.id);
        this.remember();
        show_toast({ title: "Task deleted", message: task.content, style: "success" });
      } catch (error) {
        this.tasks = [...this.tasks, task].sort(compareTasks);
        const { title, description } = problem(error);
        show_toast({ title, message: description, style: "failure" });
      }
      async_cx.notify();
    });
  }

  actions(task) {
    const priority_menu = new ActionPanelSubmenu("Change Priority").icon("flag").shortcut("secondary-shift-p").children(
      PRIORITIES.map((priority) =>
        new Action(priority.title)
          .icon(priority.value === task.priority ? "check" : "flag")
          .run((cx) => this.change(task, { priority: priority.value }, { priority: priority.value }, `Set to ${priority.title}`, cx)),
      ),
    );
    const reschedule_menu = new ActionPanelSubmenu("Reschedule").icon("calendar-clock").shortcut("secondary-shift-d").children([
      ...RESCHEDULE.map((choice) =>
        new Action(choice.title)
          .icon(choice.icon)
          .run((cx) => this.change(task, { due_string: choice.due_string }, {}, `Rescheduled to ${choice.title}`, cx)),
      ),
      new Action("Pick Date…").icon("calendar-plus").pick_date((date, cx) => {
        const day = String(date).slice(0, 10);
        this.change(task, { due_date: day }, { due: { date: day, string: "", recurring: false } }, "Rescheduled", cx);
      }),
      new Action("Remove Due Date").icon("calendar").run((cx) => this.change(task, { due_string: "no date" }, { due: null }, "Due date removed", cx)),
    ]);
    return new ActionPanel().children([
      new Action("Complete Task").icon("circle-check").run((cx) => this.complete(task, cx)),
      new Action("Open in Todoist").icon("external-link").open_url(taskUrl(task.id)),
      new ActionPanelSection("Edit").children([priority_menu, reschedule_menu]),
      new ActionPanelSection("Copy").children([
        new Action("Copy Task").icon("copy").shortcut("secondary-shift-c").copy(task.content),
        new Action("Copy Task URL").icon("link").copy(taskUrl(task.id)),
      ]),
      new ActionPanelSection("More").children([
        ...(task.project_id ? [new Action("Open Project in Todoist").icon("folder-open").open_url(projectUrl(task.project_id))] : []),
        new Action("Create Task").icon("plus").shortcut("secondary-n").launch("create-task"),
        new Action("Refresh").icon("refresh-cw").shortcut("secondary-r").run((cx) => {
          this.refresh(cx);
          cx.notify();
        }),
        new Action("Delete Task…")
          .icon("trash")
          .destructive()
          .shortcut("secondary-shift-backspace")
          .confirm("Delete this task?", `“${task.content}” and its sub-tasks will be deleted.`)
          .run((cx) => this.remove(task, cx)),
      ]),
    ]);
  }

  row(task) {
    const priority = priorityOf(task.priority);
    const project = this.project_name(task.project_id);
    let row = new ListItem(task.id, task.content)
      .icon("circle")
      .subtitle(this.group === "date" ? project : task.description.split("\n")[0])
      .keyword(project);
    if (priority.tone) row = row.icon_tone(priority.tone).tag(priority.short, priority.tone);
    if (task.description) row = row.keyword(task.description);
    for (const label of task.labels) row = row.keyword(label);
    if (task.labels.length > 0) row = row.accessory(task.labels.map((label) => `@${label}`).join(" "));
    if (task.due) {
      row = row.accessory(dueText(task)).accessory_tooltip(task.due.recurring ? `Repeats ${task.due.string}` : task.due.string || "Due");
      if (task.due.recurring) row = row.accessory_icon("refresh-cw");
    }
    return row.actions(this.actions(task));
  }

  by_date() {
    const sections = new Map([["overdue", []], ["today", []]]);
    const upcoming = new Map();
    const none = [];
    const week = isoDay(new Date(Date.now() + 7 * 24 * 3600 * 1000));
    for (const task of this.tasks) {
      const bucket = dueBucket(task);
      if (bucket === "none") none.push(task);
      else if (bucket !== "upcoming") sections.get(bucket).push(task);
      else {
        const day = task.due.date.slice(0, 10);
        const key = day <= week ? day : "later";
        upcoming.set(key, [...(upcoming.get(key) ?? []), task]);
      }
    }
    const result = [];
    if (sections.get("overdue").length > 0) result.push(["Overdue", sections.get("overdue")]);
    if (sections.get("today").length > 0) result.push(["Today", sections.get("today")]);
    for (const [key, tasks] of [...upcoming.entries()].sort(([a], [b]) => (a === "later" ? 1 : b === "later" ? -1 : a < b ? -1 : 1))) {
      result.push([key === "later" ? "Later" : dueText({ due: { date: key } }), tasks]);
    }
    if (none.length > 0) result.push(["No Date", none]);
    return result;
  }

  by_project() {
    const known = this.projects.map((project) => [project.name, this.tasks.filter((task) => task.project_id === project.id)]);
    const ids = new Set(this.projects.map((project) => project.id));
    const other = this.tasks.filter((task) => !ids.has(task.project_id));
    return [...known, ["Other", other]].filter(([, tasks]) => tasks.length > 0);
  }

  render() {
    const groups = this.group === "project" ? this.by_project() : this.by_date();
    const empty = this.error ?? (this.loading
      ? { title: "Loading tasks…", description: "" }
      : { title: "No tasks", description: "Nothing to do. Run “Create Todoist Task” to add one." });
    return new List()
      .placeholder("Search tasks…")
      .loading(this.loading)
      .empty_title(empty.title)
      .empty_description(empty.description)
      .dropdown(
        new ListDropdown("Group By")
          .value(this.group)
          .children([new ListDropdownItem("date", "By Date"), new ListDropdownItem("project", "By Project")])
          .on_change((value, cx) => {
            this.group = value === "project" ? "project" : "date";
            localStorage.setItem(GROUP_KEY, this.group);
            cx.notify();
          }),
      )
      .children(
        groups.map(([title, tasks]) =>
          new ListSection(title).subtitle(String(tasks.length)).children(tasks.map((task) => this.row(task))),
        ),
      );
  }
}
