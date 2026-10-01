// Linear's GraphQL API with a personal API key, and the queries the commands
// share. Every request is a POST to one endpoint, which is all the extension's
// network grant allows.
import { launch } from "launcher/api";

const ENDPOINT = "https://api.linear.app/graphql";

export const PRIORITIES = [
  { value: 1, title: "Urgent", tone: "danger", icon: "signal-high" },
  { value: 2, title: "High", tone: "warning", icon: "signal-high" },
  { value: 3, title: "Medium", tone: "accent", icon: "signal-medium" },
  { value: 4, title: "Low", tone: "neutral", icon: "signal-low" },
  { value: 0, title: "No Priority", tone: "neutral", icon: "ellipsis" },
];

/** State types in the order the lists show them. */
export const STATE_TYPES = [
  { type: "started", title: "In Progress", icon: "circle-dot" },
  { type: "unstarted", title: "Todo", icon: "circle" },
  { type: "triage", title: "Triage", icon: "circle-alert" },
  { type: "backlog", title: "Backlog", icon: "circle-dashed" },
  { type: "completed", title: "Done", icon: "circle-check" },
  { type: "canceled", title: "Canceled", icon: "circle-x" },
];

export const ISSUE_FIELDS = `
  id identifier title description priority priorityLabel url branchName updatedAt dueDate
  state { id name type color }
  team { id key name }
  assignee { id name displayName }
  project { name }
  labels { nodes { name } }
`;

const WORKSPACE = `
  viewer { id name }
  teams(first: 100) { nodes { id key name states { nodes { id name type position } } } }
`;

export class LinearError extends Error {}

function apiKey() {
  return String(launch().preferences.api_key ?? "").trim();
}

/** Runs a query or mutation and answers its `data`; throws a readable `LinearError`. */
export async function gql(query, variables = {}) {
  let response;
  try {
    response = await fetch(ENDPOINT, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: apiKey() },
      body: JSON.stringify({ query, variables }),
    });
  } catch (error) {
    throw new LinearError(`Cannot reach Linear: ${String(error?.message ?? error)}`);
  }
  let answer = null;
  try {
    answer = JSON.parse(await response.text());
  } catch {
    // Reported below by status.
  }
  const first = answer?.errors?.[0];
  if (first) {
    const code = first.extensions?.code ?? "";
    if (code === "AUTHENTICATION_ERROR" || response.status === 401) {
      throw new LinearError("Linear refused the API key. Check it in the extension's preferences.");
    }
    throw new LinearError(first.extensions?.userPresentableMessage ?? first.message ?? "Linear answered an error.");
  }
  if (!response.ok || !answer?.data) {
    throw new LinearError(
      response.status === 429 ? "Linear's rate limit was reached; try again shortly." : `Linear answered ${response.status}.`,
    );
  }
  return answer.data;
}

/** The signed-in user and every team with its workflow states. */
export async function workspace() {
  const data = await gql(`query Workspace { ${WORKSPACE} }`);
  return { viewer: data.viewer, teams: teamsFrom(data) };
}

function teamsFrom(data) {
  return data.teams.nodes.map((team) => ({
    ...team,
    states: [...team.states.nodes].sort((a, b) => stateOrder(a) - stateOrder(b) || a.position - b.position),
  }));
}

/** Open issues assigned to the user, and the workspace, in one request. */
export async function myIssues() {
  const data = await gql(`query MyIssues {
    ${WORKSPACE}
    me: viewer {
      assignedIssues(
        first: 100
        orderBy: updatedAt
        filter: { state: { type: { nin: ["completed", "canceled"] } } }
      ) { nodes { ${ISSUE_FIELDS} } }
    }
  }`);
  return {
    viewer: data.viewer,
    teams: teamsFrom(data),
    issues: data.me.assignedIssues.nodes,
  };
}

export async function searchIssues(term) {
  const data = await gql(
    `query SearchIssues($term: String!) {
      searchIssues(term: $term, first: 25) { nodes { ${ISSUE_FIELDS} } }
    }`,
    { term },
  );
  return data.searchIssues.nodes;
}

/** Changes an issue (`stateId`, `priority`, `assigneeId`…) and answers it as it is now. */
export async function updateIssue(id, input) {
  const data = await gql(
    `mutation UpdateIssue($id: String!, $input: IssueUpdateInput!) {
      issueUpdate(id: $id, input: $input) { success issue { ${ISSUE_FIELDS} } }
    }`,
    { id, input },
  );
  if (!data.issueUpdate.success) throw new LinearError("Linear did not update the issue.");
  return data.issueUpdate.issue;
}

export async function createIssue(input) {
  const data = await gql(
    `mutation CreateIssue($input: IssueCreateInput!) {
      issueCreate(input: $input) { success issue { id identifier title url } }
    }`,
    { input },
  );
  if (!data.issueCreate.success) throw new LinearError("Linear did not create the issue.");
  return data.issueCreate.issue;
}

export function stateOrder(state) {
  const index = STATE_TYPES.findIndex(({ type }) => type === state?.type);
  return index === -1 ? STATE_TYPES.length : index;
}

export function priorityOf(value) {
  return PRIORITIES.find((priority) => priority.value === value) ?? PRIORITIES[PRIORITIES.length - 1];
}

/** A user-facing message for anything the requests may throw. */
export function describe(error) {
  if (error instanceof LinearError) return error.message;
  const text = String(error?.message ?? error);
  return /not allow|capabilit|permission/i.test(text)
    ? "The extension may not reach Linear; allow its network access."
    : `Cannot reach Linear: ${text}`;
}
