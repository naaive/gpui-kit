// A menu-bar command: the number of tasks due today (overdue included) beside
// a tray icon, refreshed every 10 minutes. Each task in the menu completes it;
// the menu also opens Todoist and the Search Tasks command.
import { View } from "gpui-kit";
import {
  Action,
  MenuBarExtra,
  MenuBarItem,
  MenuBarSection,
  MenuBarSeparator,
} from "launcher";
import { show_toast } from "launcher/api";
import { TodoistError, closeTask, compareTasks, dueBucket, dueText, filteredTasks, priorityOf } from "../lib/todoist.js";

const MAX_SHOWN = 20;

export default class TodayTray extends View {
  init(_props, cx) {
    this.tasks = [];
    this.loading = true;
    this.error = null;
    this.load(cx);
  }

  load(cx) {
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        this.tasks = (await filteredTasks("today | overdue")).sort(compareTasks);
        this.error = null;
      } catch (error) {
        this.error = error instanceof TodoistError ? error.title : "Cannot load tasks";
      }
      this.loading = false;
      task.notify();
    });
  }

  complete(task, cx) {
    this.tasks = this.tasks.filter((each) => each.id !== task.id);
    cx.notify();
    cx.spawn(async (async_cx) => {
      try {
        await closeTask(task.id);
        // A recurring task may still be due today at a later time.
        if (task.due?.recurring) this.load(async_cx);
      } catch (error) {
        this.tasks = [...this.tasks, task].sort(compareTasks);
        show_toast({ title: "Cannot complete the task", message: String(error?.message ?? error), style: "failure" });
      }
      async_cx.notify();
    });
  }

  item(task) {
    const priority = priorityOf(task.priority);
    const when = dueBucket(task) === "overdue" ? `Overdue · ${dueText(task)}` : dueText(task);
    return new MenuBarItem(task.content)
      .subtitle(priority.tone ? `${priority.short} · ${when}` : when)
      .action(new Action("Complete Task").run((cx) => this.complete(task, cx)));
  }

  render() {
    const count = this.tasks.length;
    const overdue = this.tasks.filter((task) => dueBucket(task) === "overdue");
    const today = this.tasks.filter((task) => dueBucket(task) !== "overdue");
    const sections = [];
    if (this.error) {
      sections.push(new MenuBarItem(this.error).subtitle("Check the API token in the extension's preferences"));
    } else if (count === 0 && !this.loading) {
      sections.push(new MenuBarItem("Nothing due today"));
    } else {
      if (overdue.length > 0) sections.push(new MenuBarSection("Overdue").children(overdue.slice(0, MAX_SHOWN).map((task) => this.item(task))));
      if (today.length > 0) sections.push(new MenuBarSection("Today").children(today.slice(0, MAX_SHOWN).map((task) => this.item(task))));
    }
    return new MenuBarExtra()
      .icon("list-todo")
      .title(this.error || count === 0 ? "" : String(count))
      .tooltip(this.error ? "Todoist" : `Todoist: ${count} due today`)
      .loading(this.loading)
      .children([
        ...sections,
        new MenuBarSeparator(),
        new MenuBarItem("Search Tasks").action(new Action("Search Tasks").launch("search-tasks")),
        new MenuBarItem("Create Task").action(new Action("Create Task").launch("create-task")),
        new MenuBarItem("Open Todoist").action(new Action("Open Todoist").open_url("https://app.todoist.com/app/today")),
        new MenuBarItem("Refresh").action(new Action("Refresh").run((cx) => {
          this.load(cx);
          cx.notify();
        })),
      ]);
  }
}
