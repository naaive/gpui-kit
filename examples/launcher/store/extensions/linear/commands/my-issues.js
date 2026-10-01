// Lists the open issues assigned to you, grouped by state (in progress, todo,
// backlog…) and ordered by priority, with each issue's description beside
// the list. The last answer shows at once while a fresh one loads.
import { List, ListSection } from "launcher";
import { cache_get, cache_set } from "launcher/api";
import { IssueListView } from "../lib/issue-list.js";
import { describe, myIssues, STATE_TYPES } from "../lib/linear.js";

const CACHE_KEY = "my-issues";

function byPriority(a, b) {
  // 0 is "no priority", which sorts after Low.
  const rank = (issue) => (issue.priority === 0 ? 5 : issue.priority);
  return rank(a) - rank(b) || String(b.updatedAt).localeCompare(String(a.updatedAt));
}

function cached() {
  try {
    return cache_get(CACHE_KEY);
  } catch {
    return null;
  }
}

export default class MyIssues extends IssueListView {
  init(props, cx) {
    super.init();
    const last = cached();
    this.issues = last?.issues ?? [];
    this.viewer = last?.viewer ?? null;
    this.teams = last?.teams ?? [];
    this.loading = true;
    this.error = null;
    this.load(cx);
  }

  load(cx) {
    cx.spawn(async (task) => {
      try {
        const { viewer, teams, issues } = await myIssues();
        Object.assign(this, { viewer, teams, issues, error: null });
        try {
          cache_set(CACHE_KEY, { viewer, teams, issues });
        } catch {
          // Without a cache the list only opens a moment slower.
        }
      } catch (error) {
        this.error = describe(error);
      } finally {
        this.loading = false;
        task.notify();
      }
    });
  }

  replaceIssue(issue) {
    super.replaceIssue(issue);
    // Reassigned to someone else: no longer one of mine.
    if (this.viewer && issue.assignee?.id !== this.viewer.id) {
      this.issues = this.issues.filter((each) => each.id !== issue.id);
    }
  }

  sections() {
    return STATE_TYPES.map(({ type, title }) => {
      const issues = this.issues.filter((issue) => issue.state?.type === type).sort(byPriority);
      return issues.length === 0
        ? null
        : new ListSection(title).subtitle(String(issues.length)).children(issues.map((issue) => this.issueRow(issue)));
    }).filter(Boolean);
  }

  render() {
    const [title, description] = this.error
      ? ["Cannot load your issues", this.error]
      : this.loading
        ? ["Loading your issues…", "Asking Linear"]
        : ["No open issues", "Nothing is assigned to you. Enjoy it."];
    return new List()
      .placeholder("Filter my issues…")
      .loading(this.loading)
      .showing_detail(this.issues.length > 0)
      .empty_title(title)
      .empty_description(description)
      .children(this.sections());
  }
}
