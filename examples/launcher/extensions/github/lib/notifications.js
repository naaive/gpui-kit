// Unread notifications, shared by the Notifications list and the tray menu.
// Marking read is a PATCH for one thread, or a PUT for all of them.
import { rest } from "./github.js";

export const INBOX_URL = "https://github.com/notifications";

/** `PullRequest` → "Pull Request". */
const TYPES = {
  PullRequest: ["Pull Request", "git-pull-request"],
  Issue: ["Issue", "circle-dot"],
  Commit: ["Commit", "git-commit-horizontal"],
  Release: ["Release", "tag"],
  Discussion: ["Discussion", "message-circle"],
  CheckSuite: ["Workflow Run", "play"],
  RepositoryVulnerabilityAlert: ["Security Alert", "shield-alert"],
};

const REASONS = {
  assign: "Assigned",
  author: "Author",
  comment: "Commented",
  ci_activity: "CI",
  invitation: "Invitation",
  manual: "Subscribed",
  mention: "Mentioned",
  review_requested: "Review Requested",
  security_alert: "Security",
  state_change: "State Changed",
  subscribed: "Watching",
  team_mention: "Team Mentioned",
};

/**
 * The github.com page of a notification's subject. The API names it by its
 * REST URL (`api.github.com/repos/o/r/pulls/1`), or not at all for a
 * discussion or a workflow run.
 */
export function htmlUrl(subject, repository) {
  const base = repository.html_url;
  const match = /^https:\/\/api\.github\.com\/repos\/([^/]+\/[^/]+)\/(pulls|issues|commits|releases)\/([^/]+)$/.exec(
    subject.url ?? "",
  );
  if (match) {
    const [, name, kind, id] = match;
    if (kind === "pulls") return `https://github.com/${name}/pull/${id}`;
    if (kind === "issues") return `https://github.com/${name}/issues/${id}`;
    if (kind === "commits") return `https://github.com/${name}/commit/${id}`;
    return `https://github.com/${name}/releases`;
  }
  if (subject.type === "Discussion") return `${base}/discussions`;
  if (subject.type === "CheckSuite") return `${base}/actions`;
  if (subject.type === "RepositoryVulnerabilityAlert") return `${base}/security`;
  return base;
}

/** The unread notifications, newest first. */
export async function loadNotifications(secret) {
  const threads = (await rest("/notifications?per_page=50", secret)) ?? [];
  return threads.map((thread) => {
    const [type, icon] = TYPES[thread.subject.type] ?? [thread.subject.type, "bell"];
    return {
      id: String(thread.id),
      title: thread.subject.title,
      type,
      icon,
      reason: REASONS[thread.reason] ?? thread.reason,
      repository: thread.repository.full_name,
      updated: thread.updated_at,
      url: htmlUrl(thread.subject, thread.repository),
    };
  });
}

/** Marks one thread, or with `all` every notification, read on GitHub. */
export async function markRead(secret, threads, all = false) {
  if (all) {
    await rest("/notifications", secret, { method: "PUT", body: { read: true } });
    return;
  }
  for (const thread of threads) {
    await rest(`/notifications/threads/${thread.id}`, secret, { method: "PATCH" });
  }
}
