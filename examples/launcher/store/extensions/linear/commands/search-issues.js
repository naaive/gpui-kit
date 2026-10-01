// Searches every issue in the workspace as you type (Linear's `searchIssues`
// full-text search), with the same detail and actions as My Issues.
import { List } from "launcher";
import { launch } from "launcher/api";
import { IssueListView } from "../lib/issue-list.js";
import { describe, searchIssues, workspace } from "../lib/linear.js";

const TYPING_PAUSE_MS = 300;

export default class SearchIssues extends IssueListView {
  init(props, cx) {
    super.init();
    this.issues = [];
    this.query = "";
    this.loading = false;
    this.error = null;
    this.generation = 0;
    this.pending = null;
    this.loadWorkspace(cx);
    const query = String(launch().arguments.query ?? "").trim();
    if (query) {
      this.query = query;
      this.search(cx);
    }
  }

  /** The user and the teams' states, for Assign to Me and Change Status; a failure here is not shown. */
  loadWorkspace(cx) {
    cx.spawn(async (task) => {
      try {
        const { viewer, teams } = await workspace();
        this.viewer = viewer;
        this.teams = teams;
        task.notify();
      } catch {
        // The search reports what is wrong.
      }
    });
  }

  typed(query, cx) {
    this.query = query.trim();
    this.pending?.cancel();
    this.generation += 1;
    this.error = null;
    this.loading = this.query !== "";
    if (!this.query) this.issues = [];
    this.pending = this.query ? cx.timer.after(TYPING_PAUSE_MS, (timer) => this.search(timer)) : null;
    cx.notify();
  }

  search(cx) {
    const run = ++this.generation;
    const query = this.query;
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        const issues = await searchIssues(query);
        if (run !== this.generation) return;
        this.issues = issues;
        this.error = null;
      } catch (error) {
        if (run !== this.generation) return;
        this.issues = [];
        this.error = describe(error);
      } finally {
        if (run === this.generation) {
          this.loading = false;
          task.notify();
        }
      }
    });
  }

  render() {
    const [title, description] = this.error
      ? ["Cannot search Linear", this.error]
      : this.query
        ? ["No issues found", "Try other words or an issue ID."]
        : ["Search Linear Issues", "Type words from a title or description, or an issue ID."];
    return new List()
      .placeholder("Search issues…")
      .loading(this.loading)
      .showing_detail(this.issues.length > 0)
      .empty_title(this.loading ? "Searching…" : title)
      .empty_description(this.loading ? `Looking for “${this.query}”` : description)
      .on_query_change((query, cx) => this.typed(query, cx))
      .children(this.issues.map((issue) => this.issueRow(issue)));
  }
}
