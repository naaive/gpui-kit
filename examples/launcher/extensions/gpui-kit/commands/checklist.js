// A stateful page: the View keeps its own state, `run` actions call back into
// it, and `cx.notify()` asks the launcher to render the page again. Items keep
// their ids across renders, so the selection stays on the task just toggled.
import { View } from "gpui-kit";
import { Action, List, ListItem } from "launcher";
import { launch, show_toast } from "launcher/api";

export default class Checklist extends View {
  init() {
    this.command = launch().command;
    this.tasks = [
      { id: "install", title: "Install Rust", done: true },
      { id: "run", title: "Run the Story gallery", done: false },
      { id: "guides", title: "Read the Design Guides", done: false },
      { id: "extension", title: "Write a launcher extension", done: false },
    ];
  }

  toggle(task, cx) {
    task.done = !task.done;
    if (this.tasks.every((each) => each.done)) {
      show_toast("Everything is done", "success");
    }
    cx.notify();
  }

  row(task) {
    const item = new ListItem(task.id, task.title)
      .icon(task.done ? "circle-check" : "dash")
      .action(
        new Action(task.done ? "Mark as Not Done" : "Mark as Done").run((cx) => this.toggle(task, cx)),
      )
      .action(new Action("Copy Title").shortcut("secondary-shift-c").copy(task.title));
    return task.done ? item.accessory("Done") : item;
  }

  render() {
    const remaining = this.tasks.filter((task) => !task.done).length;
    return new List()
      .placeholder(`Filter tasks (${remaining} remaining)…`)
      .empty_title("No matching task")
      .children(this.tasks.map((task) => this.row(task)));
  }
}
