// The open pull requests that concern you, in three sections: the ones you
// opened, the ones waiting for your review, and the ones assigned to you. One
// GraphQL request answers all three, with each pull request's branch, draft
// state and CI status. Without a token the page says how to add one.
import { View } from "gpui-kit";
import { Action, ActionPanel, ActionPanelSection, List, ListItem, ListSection } from "launcher";
import { Query } from "launcher/utils";
import { NO_TOKEN, graphql, messageOf, noTokenItem, reference, token, viewerLogin } from "../lib/github.js";

const SECTIONS = [
  { key: "authored", title: "Created by Me", qualifier: "author" },
  { key: "review", title: "Review Requested", qualifier: "review-requested" },
  { key: "assigned", title: "Assigned to Me", qualifier: "assignee" },
];

const QUERY = `
  query ($authored: String!, $review: String!, $assigned: String!) {
    authored: search(query: $authored, type: ISSUE, first: 30) { nodes { ...pull } }
    review: search(query: $review, type: ISSUE, first: 30) { nodes { ...pull } }
    assigned: search(query: $assigned, type: ISSUE, first: 30) { nodes { ...pull } }
  }
  fragment pull on PullRequest {
    id number title url isDraft updatedAt headRefName
    repository { nameWithOwner }
    author { login }
    commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
  }
`;

// What a CI state looks like as a trailing tag.
const CHECKS = {
  SUCCESS: ["Passing", "success"],
  FAILURE: ["Failing", "danger"],
  ERROR: ["Failing", "danger"],
  PENDING: ["Running", "warning"],
  EXPECTED: ["Running", "warning"],
};

function searchFor(login, qualifier) {
  return `is:pr is:open archived:false ${qualifier}:${login} sort:updated-desc`;
}

async function loadPullRequests(secret) {
  const login = await viewerLogin(secret);
  const variables = Object.fromEntries(SECTIONS.map((section) => [section.key, searchFor(login, section.qualifier)]));
  const data = await graphql(QUERY, variables, secret);
  // A pull request shows once, in the first section that has it.
  const seen = new Set();
  return SECTIONS.map((section) => ({
    title: section.title,
    pulls: data[section.key].nodes
      .filter((pull) => pull?.id && !seen.has(pull.id) && seen.add(pull.id))
      .map((pull) => ({
        id: pull.id,
        number: pull.number,
        title: pull.title,
        url: pull.url,
        draft: pull.isDraft,
        updated: pull.updatedAt,
        branch: pull.headRefName,
        repository: pull.repository.nameWithOwner,
        author: pull.author?.login ?? "ghost",
        checks: pull.commits.nodes[0]?.commit.statusCheckRollup?.state ?? null,
      })),
  }));
}

function row(pull) {
  let item = new ListItem(pull.id, pull.title)
    .icon(pull.draft ? "git-pull-request-draft" : "git-pull-request")
    .icon_tone(pull.draft ? "neutral" : "success")
    .subtitle(reference(pull.repository, pull.number))
    .keyword(pull.repository)
    .keyword(String(pull.number))
    .keyword(pull.branch);
  if (pull.draft) item = item.tag("Draft");
  const checks = CHECKS[pull.checks];
  if (checks) item = item.tag(checks[0], checks[1]).accessory_tooltip("CI status of the latest commit");
  return item
    .accessory_date(pull.updated)
    .actions(
      new ActionPanel().children([
        new Action("Open in Browser").open_url(pull.url),
        new Action("Copy URL").icon("copy").shortcut("secondary-shift-c").copy(pull.url),
        new Action("Copy Branch Name").icon("git-branch").shortcut("secondary-shift-b").copy(pull.branch),
        new Action("Copy Checkout Command")
          .icon("terminal")
          .shortcut("secondary-shift-o")
          .copy(`gh pr checkout ${pull.number} --repo ${pull.repository}`),
        new ActionPanelSection("More").children([
          new Action("Open Files Changed").icon("external-link").open_url(`${pull.url}/files`),
          new Action("Open Checks").icon("circle-check").open_url(`${pull.url}/checks`),
          new Action("Copy Reference").copy(reference(pull.repository, pull.number)),
        ]),
      ]),
    );
}

export default class MyPullRequests extends View {
  init(props, cx) {
    this.token = token();
    if (!this.token) return;
    const secret = this.token;
    this.pulls = new Query(cx, () => loadPullRequests(secret), {
      cache_key: "my-pull-requests",
      failure_title: "Cannot load pull requests",
    });
  }

  emptyState() {
    if (this.pulls.loading && !this.pulls.data) return ["Loading pull requests…", "Asking GitHub"];
    if (this.pulls.error) return ["Cannot load pull requests", messageOf(this.pulls.error)];
    return ["No open pull requests", "Nothing you opened, were asked to review, or were assigned is open."];
  }

  render() {
    if (!this.token) {
      return new List().empty_title(NO_TOKEN.title).empty_description(NO_TOKEN.description).child(noTokenItem());
    }
    const [title, description] = this.emptyState();
    const reload = new Action("Reload").icon("refresh-cw").shortcut("secondary-r").run((cx) => this.pulls.revalidate(cx));
    const sections = (this.pulls.data ?? [])
      .filter((section) => section.pulls.length > 0)
      .map((section) =>
        new ListSection(section.title)
          .subtitle(String(section.pulls.length))
          .children(section.pulls.map((pull) => row(pull).action(reload))),
      );
    return new List()
      .placeholder("Filter pull requests…")
      .loading(this.pulls.loading)
      .empty_title(title)
      .empty_description(description)
      .children(sections);
  }
}
