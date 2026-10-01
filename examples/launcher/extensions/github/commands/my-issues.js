// The open issues that concern you, in three sections: assigned to you,
// opened by you, and mentioning you. One GraphQL request answers all three.
// Without a token the page says how to add one.
import { View } from "gpui-kit";
import { Action, ActionPanel, ActionPanelSection, List, ListItem, ListSection } from "launcher";
import { Query } from "launcher/utils";
import { NO_TOKEN, graphql, messageOf, noTokenItem, reference, token, viewerLogin } from "../lib/github.js";

const SECTIONS = [
  { key: "assigned", title: "Assigned to Me", qualifier: "assignee" },
  { key: "created", title: "Created by Me", qualifier: "author" },
  { key: "mentioned", title: "Mentioning Me", qualifier: "mentions" },
];

const QUERY = `
  query ($assigned: String!, $created: String!, $mentioned: String!) {
    assigned: search(query: $assigned, type: ISSUE, first: 30) { nodes { ...issue } }
    created: search(query: $created, type: ISSUE, first: 30) { nodes { ...issue } }
    mentioned: search(query: $mentioned, type: ISSUE, first: 30) { nodes { ...issue } }
  }
  fragment issue on Issue {
    id number title url updatedAt
    repository { nameWithOwner }
    comments { totalCount }
    labels(first: 3) { nodes { name } }
  }
`;

function searchFor(login, qualifier) {
  return `is:issue is:open archived:false ${qualifier}:${login} sort:updated-desc`;
}

async function loadIssues(secret) {
  const login = await viewerLogin(secret);
  const variables = Object.fromEntries(SECTIONS.map((section) => [section.key, searchFor(login, section.qualifier)]));
  const data = await graphql(QUERY, variables, secret);
  // An issue shows once, in the first section that has it.
  const seen = new Set();
  return SECTIONS.map((section) => ({
    title: section.title,
    issues: data[section.key].nodes
      .filter((issue) => issue?.id && !seen.has(issue.id) && seen.add(issue.id))
      .map((issue) => ({
        id: issue.id,
        number: issue.number,
        title: issue.title,
        url: issue.url,
        updated: issue.updatedAt,
        repository: issue.repository.nameWithOwner,
        comments: issue.comments.totalCount,
        labels: issue.labels.nodes.map((label) => label.name),
      })),
  }));
}

function row(issue) {
  let item = new ListItem(issue.id, issue.title)
    .icon("circle-dot")
    .icon_tone("success")
    .subtitle(reference(issue.repository, issue.number))
    .keyword(issue.repository)
    .keyword(String(issue.number));
  for (const label of issue.labels) item = item.tag(label).keyword(label);
  if (issue.comments > 0) {
    item = item.accessory_icon("message-square").accessory(String(issue.comments)).accessory_tooltip("Comments");
  }
  const ref = reference(issue.repository, issue.number);
  return item.accessory_date(issue.updated).actions(
    new ActionPanel().children([
      new Action("Open in Browser").open_url(issue.url),
      new Action("Copy URL").icon("copy").shortcut("secondary-shift-c").copy(issue.url),
      new Action("Copy Reference").icon("copy").shortcut("secondary-shift-r").copy(ref),
      new ActionPanelSection("More").children([
        new Action("Copy Title").copy(issue.title),
        new Action("Copy as Markdown Link").copy(`[${ref}: ${issue.title}](${issue.url})`),
      ]),
    ]),
  );
}

export default class MyIssues extends View {
  init(props, cx) {
    this.token = token();
    if (!this.token) return;
    const secret = this.token;
    this.issues = new Query(cx, () => loadIssues(secret), {
      cache_key: "my-issues",
      failure_title: "Cannot load issues",
    });
  }

  emptyState() {
    if (this.issues.loading && !this.issues.data) return ["Loading issues…", "Asking GitHub"];
    if (this.issues.error) return ["Cannot load issues", messageOf(this.issues.error)];
    return ["No open issues", "Nothing assigned to you, opened by you, or mentioning you is open."];
  }

  render() {
    if (!this.token) {
      return new List().empty_title(NO_TOKEN.title).empty_description(NO_TOKEN.description).child(noTokenItem());
    }
    const [title, description] = this.emptyState();
    const reload = new Action("Reload").icon("refresh-cw").shortcut("secondary-r").run((cx) => this.issues.revalidate(cx));
    const sections = (this.issues.data ?? [])
      .filter((section) => section.issues.length > 0)
      .map((section) =>
        new ListSection(section.title)
          .subtitle(String(section.issues.length))
          .children(section.issues.map((issue) => row(issue).action(reload))),
      );
    return new List()
      .placeholder("Filter issues…")
      .loading(this.issues.loading)
      .empty_title(title)
      .empty_description(description)
      .children(sections);
  }
}
