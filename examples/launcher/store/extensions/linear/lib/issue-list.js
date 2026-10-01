// What My Issues and Search Issues share: a row per issue with its detail,
// and the actions that open, copy and change it. A subclass loads the
// issues and renders the `List`; this class keeps the workspace (the user and
// each team's workflow states) that the Change Status and Assign to Me
// actions need.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  ActionPanelSubmenu,
  Detail,
  ListItem,
  MetadataLabel,
  MetadataLink,
  MetadataSeparator,
  MetadataTags,
} from "launcher";
import { show_toast } from "launcher/api";
import { describe, PRIORITIES, priorityOf, STATE_TYPES, updateIssue } from "./linear.js";

export function stateIcon(state) {
  return STATE_TYPES.find(({ type }) => type === state?.type)?.icon ?? "circle";
}

function formatDate(iso) {
  return iso ? String(iso).slice(0, 10) : "";
}

export function issueDetail(issue) {
  const priority = priorityOf(issue.priority);
  const labels = issue.labels?.nodes ?? [];
  const body = issue.description?.trim() ? issue.description : "_No description_";
  return new Detail(`# ${issue.title}\n\n${body}`).children([
    new MetadataLabel("Issue", issue.identifier),
    new MetadataTags("Status").tag(issue.state?.name ?? "Unknown", issue.state?.type === "started" ? "accent" : "neutral"),
    new MetadataTags("Priority").tag(priority.title, priority.tone),
    new MetadataLabel("Team", issue.team ? `${issue.team.name} (${issue.team.key})` : "None"),
    new MetadataLabel("Assignee", issue.assignee?.displayName ?? issue.assignee?.name ?? "Unassigned"),
    ...(issue.project ? [new MetadataLabel("Project", issue.project.name)] : []),
    ...(labels.length > 0
      ? [labels.reduce((tags, label) => tags.tag(label.name), new MetadataTags("Labels"))]
      : []),
    ...(issue.dueDate ? [new MetadataLabel("Due", formatDate(issue.dueDate))] : []),
    new MetadataLabel("Updated", formatDate(issue.updatedAt)),
    new MetadataSeparator(),
    new MetadataLink("Open", issue.identifier, issue.url),
  ]);
}

export class IssueListView extends View {
  init() {
    this.viewer = null;
    this.teams = [];
  }

  /** Puts a changed issue in place of the old one; a subclass may also move or drop it. */
  replaceIssue(issue) {
    this.issues = (this.issues ?? []).map((each) => (each.id === issue.id ? issue : each));
  }

  change(cx, issue, input, message) {
    show_toast({ title: "Updating…", message: issue.identifier, style: "progress", id: issue.id });
    cx.spawn(async (task) => {
      try {
        const updated = await updateIssue(issue.id, input);
        this.replaceIssue(updated);
        show_toast({ title: message(updated), message: updated.identifier, style: "success", id: issue.id });
        task.notify();
      } catch (error) {
        show_toast({ title: "Cannot update the issue", message: describe(error), style: "failure", id: issue.id });
      }
    });
  }

  statusMenu(issue) {
    const team = this.teams.find((each) => each.id === issue.team?.id);
    const states = team?.states ?? [];
    return new ActionPanelSubmenu("Change Status")
      .icon("circle-dot")
      .shortcut("secondary-shift-s")
      .children(
        states.length > 0
          ? states.map((state) =>
              new Action(state.id === issue.state?.id ? `${state.name} (Current)` : state.name)
                .icon(stateIcon(state))
                .run((cx) => this.change(cx, issue, { stateId: state.id }, (updated) => `Moved to ${updated.state.name}`)),
            )
          : [new Action("Statuses Are Still Loading").toast("Try again in a moment", "info")],
      );
  }

  priorityMenu(issue) {
    return new ActionPanelSubmenu("Change Priority")
      .icon("signal-high")
      .shortcut("secondary-shift-p")
      .children(
        PRIORITIES.map((priority) =>
          new Action(priority.value === issue.priority ? `${priority.title} (Current)` : priority.title)
            .icon(priority.icon)
            .run((cx) =>
              this.change(cx, issue, { priority: priority.value }, () => `Priority set to ${priority.title}`),
            ),
        ),
      );
  }

  issueActions(issue) {
    const mine = this.viewer && issue.assignee?.id === this.viewer.id;
    return new ActionPanel().children([
      new Action("Open in Linear").icon("external-link").open_url(issue.url),
      new Action("Copy Git Branch Name").icon("git-branch").shortcut("secondary-shift-b").copy(issue.branchName),
      new ActionPanelSection("Copy").children([
        new Action("Copy Issue ID").icon("copy").shortcut("secondary-shift-c").copy(issue.identifier),
        new Action("Copy URL").icon("link").shortcut("secondary-shift-u").copy(issue.url),
        new Action("Copy Title").icon("copy").copy(issue.title),
      ]),
      new ActionPanelSection("Change").children([
        this.statusMenu(issue),
        this.priorityMenu(issue),
        ...(this.viewer && !mine
          ? [
              new Action("Assign to Me")
                .icon("user-check")
                .shortcut("secondary-shift-i")
                .run((cx) => this.change(cx, issue, { assigneeId: this.viewer.id }, () => "Assigned to you")),
            ]
          : []),
      ]),
    ]);
  }

  issueRow(issue) {
    const priority = priorityOf(issue.priority);
    let row = new ListItem(issue.id, issue.title)
      .icon(stateIcon(issue.state))
      .subtitle(issue.identifier)
      .keyword(issue.identifier);
    if (issue.state?.name) row = row.keyword(issue.state.name);
    if (issue.priority > 0) row = row.tag(priority.title, priority.tone);
    if (issue.team?.key) row = row.accessory(issue.team.key).accessory_tooltip(issue.team.name);
    return row.detail(issueDetail(issue)).actions(this.issueActions(issue));
  }
}
