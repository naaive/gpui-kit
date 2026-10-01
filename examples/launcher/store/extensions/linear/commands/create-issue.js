// A form that creates a Linear issue: team, title, description, priority,
// and optionally assigns it to you. The toast it shows offers to open the
// new issue. The team chosen last is chosen again next time.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  Checkbox,
  Dropdown,
  DropdownItem,
  Form,
  FormDescription,
  TextArea,
  TextField,
} from "launcher";
import { launch, open, show_toast } from "launcher/api";
import { FormState } from "launcher/utils";
import { createIssue, describe, PRIORITIES, workspace } from "../lib/linear.js";

const LAST_TEAM_KEY = "last-team";

function lastTeam() {
  try {
    return localStorage.getItem(LAST_TEAM_KEY);
  } catch {
    return null;
  }
}

function blank(team = null) {
  return { team, title: "", description: "", priority: "0", assign: true };
}

export default class CreateIssue extends View {
  init(props, cx) {
    this.teams = [];
    this.viewer = null;
    this.loading = true;
    this.saving = false;
    this.error = null;
    this.form = new FormState(
      { ...blank(), title: String(launch().arguments.title ?? "") },
      { team: FormState.required("Choose a team"), title: FormState.required("Give the issue a title") },
    );
    this.load(cx);
  }

  load(cx) {
    cx.spawn(async (task) => {
      try {
        const { viewer, teams } = await workspace();
        this.viewer = viewer;
        this.teams = teams;
        const remembered = teams.find((team) => team.id === lastTeam()) ?? teams[0];
        if (remembered && !this.form.value("team")) this.form.set("team", remembered.id);
        this.error = teams.length === 0 ? "You are not a member of any team." : null;
      } catch (error) {
        this.error = describe(error);
      } finally {
        this.loading = false;
        task.notify();
      }
    });
  }

  submit(values, cx) {
    if (this.saving) return;
    if (!this.form.validate(values)) {
      cx.notify();
      return;
    }
    const { team, title, description, priority, assign } = this.form.values;
    const input = { teamId: team, title: title.trim(), priority: Number(priority ?? 0) };
    if (String(description ?? "").trim()) input.description = description;
    if (assign && this.viewer) input.assigneeId = this.viewer.id;
    this.saving = true;
    cx.notify();
    show_toast({ title: "Creating issue…", style: "progress", id: "create" });
    cx.spawn(async (task) => {
      try {
        const issue = await createIssue(input);
        try {
          localStorage.setItem(LAST_TEAM_KEY, team);
        } catch {
          // Only a convenience.
        }
        this.form.reset(blank(team));
        this.saving = false;
        task.notify();
        const choice = await show_toast({
          title: `Created ${issue.identifier}`,
          message: issue.title,
          style: "success",
          id: "create",
          primary_action: "Open",
        });
        if (choice === "primary") open(issue.url);
      } catch (error) {
        this.saving = false;
        task.notify();
        show_toast({ title: "Cannot create the issue", message: describe(error), style: "failure", id: "create" });
      }
    });
  }

  teamField() {
    const field = new Dropdown("team", "Team").children(
      this.teams.map((team) => new DropdownItem(team.id, `${team.name} (${team.key})`)),
    );
    // A dropdown's value must be one of its items: bind it only once the teams are in.
    return this.teams.some((team) => team.id === this.form.value("team")) ? this.form.bind("team", field) : field;
  }

  render() {
    const fields = [
      ...(this.error ? [new FormDescription("Linear", this.error)] : []),
      this.teamField(),
      this.form.bind("title", new TextField("title", "Title").placeholder("Issue title")),
      this.form.bind("description", new TextArea("description", "Description").placeholder("Markdown supported")),
      this.form.bind(
        "priority",
        new Dropdown("priority", "Priority").children(
          PRIORITIES.map((priority) => new DropdownItem(String(priority.value), priority.title)),
        ),
      ),
      this.form.bind("assign", new Checkbox("assign", "Assignee", "Assign to me")),
    ];
    return new Form()
      .loading(this.loading || this.saving)
      .actions(
        new ActionPanel().children([
          new Action("Create Issue").icon("plus").submit((values, cx) => this.submit(values, cx)),
          new Action("Open Linear").icon("external-link").open_url("https://linear.app"),
        ]),
      )
      .children(fields);
  }
}
