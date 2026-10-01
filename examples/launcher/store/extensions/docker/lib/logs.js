// The last lines a container printed, pushed from "View Logs". Docker hands
// back the container's standard output and standard error separately, so
// the two are shown one after the other rather than interleaved.
import { View } from "gpui-kit";
import { Action, ActionPanel, Detail, MetadataLabel } from "launcher";
import { describe, invoke } from "./docker.js";

const LINES = 200;

/** A fence longer than any run of backticks in `text`, so logs cannot close it. */
function fenced(text) {
  const longest = Math.max(2, ...(text.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(longest + 1);
  return `${fence}text\n${text}\n${fence}`;
}

function lastLines(text, count) {
  const lines = text.replace(/\s+$/, "").split(/\r?\n/);
  return lines.slice(Math.max(0, lines.length - count)).join("\n");
}

export class ContainerLogs extends View {
  init({ id, name }, cx) {
    this.cx = cx;
    this.id = id;
    this.name = name;
    this.stdout = "";
    this.stderr = "";
    this.error = null;
    this.loading = true;
    this.updated = null;
    this.reload();
  }

  reload() {
    this.loading = true;
    this.cx.spawn(async (task) => {
      try {
        const output = await invoke(["logs", "--tail", String(LINES), this.id]);
        this.stdout = lastLines(output.stdout ?? "", LINES);
        this.stderr = lastLines(output.stderr ?? "", LINES);
        this.error = null;
        this.updated = new Date();
      } catch (error) {
        const [title, message] = describe(error);
        this.error = `**${title}.** ${message}`;
      } finally {
        this.loading = false;
        task.notify();
      }
    });
  }

  heading() {
    return `# Logs of ${this.name.replace(/_/g, "\\_")}`;
  }

  markdown() {
    if (this.error) return `${this.heading()}\n\n${this.error}`;
    if (this.loading && !this.updated) return `${this.heading()}\n\nReading logs…`;
    const parts = [this.heading()];
    if (!this.stdout && !this.stderr) parts.push("The container has not printed anything.");
    if (this.stdout) parts.push(fenced(this.stdout));
    if (this.stderr) parts.push("## Standard Error", fenced(this.stderr));
    return parts.join("\n\n");
  }

  render() {
    const all = [this.stdout, this.stderr].filter(Boolean).join("\n");
    const count = all ? all.split("\n").length : 0;
    const actions = [
      new Action("Refresh Logs").icon("refresh-cw").shortcut("secondary-r").run((cx) => {
        this.reload();
        cx.notify();
      }),
    ];
    if (all) actions.push(new Action("Copy Logs").icon("copy").shortcut("secondary-shift-c").copy(all));
    return new Detail(this.markdown())
      .loading(this.loading)
      .actions(new ActionPanel().children(actions))
      .children([
        new MetadataLabel("Container", this.name),
        new MetadataLabel("Lines", `${count} (last ${LINES} of each stream)`),
        new MetadataLabel("Updated", this.updated ? this.updated.toLocaleTimeString() : "Not yet"),
      ]);
  }
}
