// What the GitHub commands share: the token, requests to the REST and
// GraphQL APIs under the grants `gpui-shell.json` asks for, and the page
// shown while no token is set.
import { createHash } from "crypto";
import { Action, ActionPanel, ListItem } from "launcher";
import { launch } from "launcher/api";

const API = "https://api.github.com";
const TOKEN_URL =
  "https://github.com/settings/tokens/new?scopes=repo,notifications&description=GPUI%20Kit%20Launcher";

/** The personal access token from the extension's preferences, or "". */
export function token() {
  const value = launch().preferences.token;
  return typeof value === "string" ? value.trim() : "";
}

function headers(secret) {
  return {
    Accept: "application/vnd.github+json",
    Authorization: `Bearer ${secret}`,
    "X-GitHub-Api-Version": "2022-11-28",
  };
}

function failure(status) {
  if (status === 401) return "GitHub refused the token. Check it in the extension's preferences.";
  if (status === 403) return "GitHub refused the request: the token may lack a scope, or the rate limit was reached.";
  if (status === 404) return "Not found. The token may lack the scope this needs.";
  return `GitHub answered ${status}.`;
}

/** A REST call; answers the parsed JSON, or `null` for an empty answer. */
export async function rest(path, secret, options = {}) {
  const response = await fetch(`${API}${path}`, {
    method: options.method ?? "GET",
    headers: { ...headers(secret), ...(options.body ? { "Content-Type": "application/json" } : {}) },
    ...(options.body ? { body: JSON.stringify(options.body) } : {}),
  });
  if (!response.ok) throw new Error(failure(response.status));
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

/** A GraphQL query; answers its `data`. */
export async function graphql(query, variables, secret) {
  const response = await fetch(`${API}/graphql`, {
    method: "POST",
    headers: { ...headers(secret), "Content-Type": "application/json" },
    body: JSON.stringify({ query, variables }),
  });
  if (!response.ok) throw new Error(failure(response.status));
  const { data, errors } = await response.json();
  if (errors?.length) throw new Error(errors[0].message);
  return data;
}

/**
 * The login the token belongs to. Kept in `localStorage` under a hash of the
 * token, so a new token asks again and the token itself is never stored.
 */
export async function viewerLogin(secret) {
  const key = `login:${createHash("sha256").update(secret).digest("hex").slice(0, 16)}`;
  const known = localStorage.getItem(key);
  if (known) return known;
  const { viewer } = await graphql("query { viewer { login } }", {}, secret);
  localStorage.setItem(key, viewer.login);
  return viewer.login;
}

export function messageOf(error) {
  return String(error?.message ?? error);
}

/** The page's empty state while no token is set. */
export const NO_TOKEN = {
  title: "Add a token in preferences",
  description: "This command reads your GitHub account with a personal access token.",
};

/** The one row shown while no token is set: create one, then add it. */
export function noTokenItem() {
  return new ListItem("add-token", "Add a Token in Preferences")
    .icon("key-round")
    .subtitle("Create a token on GitHub, then paste it into the preferences")
    .actions(
      new ActionPanel().children([
        new Action("Open Preferences").icon("settings").open_preferences(),
        new Action("Create Token on GitHub").icon("external-link").open_url(TOKEN_URL),
      ]),
    );
}

/** `octo/repo#12`. */
export function reference(repository, number) {
  return `${repository}#${number}`;
}
