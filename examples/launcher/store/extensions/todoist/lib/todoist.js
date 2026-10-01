// A small client for Todoist's API v1 (`https://api.todoist.com/api/v1`),
// which replaced REST v2 (that one now answers 410 Gone). Every call sends the
// personal API token as a Bearer token; the token is never logged.
import { launch } from "launcher/api";

const API = "https://api.todoist.com/api/v1";
const PAGE_LIMIT = 200;
const MAX_PAGES = 10;

export class TodoistError extends Error {
  constructor(title, message) {
    super(message);
    this.title = title;
  }
}

function token() {
  return String(launch().preferences.token ?? "").trim();
}

async function request(method, path, { query = null, body = null } = {}) {
  const secret = token();
  if (!secret) throw new TodoistError("No API token", "Add your Todoist API token in the extension's preferences.");
  const search = query
    ? `?${Object.entries(query).map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`).join("&")}`
    : "";
  const headers = { Authorization: `Bearer ${secret}` };
  if (body) headers["Content-Type"] = "application/json";
  let response;
  try {
    response = await fetch(`${API}${path}${search}`, { method, headers, body: body ? JSON.stringify(body) : undefined });
  } catch (error) {
    throw new TodoistError("Cannot reach Todoist", String(error?.message ?? error));
  }
  if (response.status === 401 || response.status === 403) {
    throw new TodoistError("Todoist refused the API token", "Copy the token again from Todoist Settings → Integrations → Developer.");
  }
  if (response.status === 429) throw new TodoistError("Too many requests", "Todoist asks to wait a moment; try again soon.");
  if (!response.ok) {
    let message = `Todoist answered ${response.status}.`;
    try {
      const answer = await response.json();
      if (answer?.error) message = String(answer.error);
    } catch (_) {
      // Not JSON: keep the status.
    }
    throw new TodoistError("Todoist request failed", message);
  }
  if (response.status === 204) return null;
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

/** Every page of a `{ results, next_cursor }` list. */
async function all(path, query = {}) {
  const results = [];
  let cursor = null;
  for (let page = 0; page < MAX_PAGES; page += 1) {
    const answer = await request("GET", path, { query: { ...query, limit: String(PAGE_LIMIT), ...(cursor ? { cursor } : {}) } });
    results.push(...(answer?.results ?? []));
    cursor = answer?.next_cursor ?? null;
    if (!cursor) break;
  }
  return results;
}

/** The fields the commands use, from an API task. */
function task(raw) {
  return {
    id: String(raw.id),
    content: raw.content ?? "",
    description: raw.description ?? "",
    project_id: raw.project_id ? String(raw.project_id) : null,
    priority: raw.priority ?? 1,
    labels: raw.labels ?? [],
    due: raw.due ? { date: raw.due.date, string: raw.due.string ?? "", recurring: Boolean(raw.due.is_recurring) } : null,
    order: raw.child_order ?? 0,
  };
}

export async function activeTasks() {
  return (await all("/tasks")).map(task);
}

/** Tasks matching a Todoist filter, such as `today | overdue`. */
export async function filteredTasks(filter) {
  return (await all("/tasks/filter", { query: filter })).map(task);
}

export async function projects() {
  return (await all("/projects")).map((project) => ({
    id: String(project.id),
    name: project.name ?? "Untitled",
    inbox: Boolean(project.inbox_project),
    order: project.child_order ?? 0,
  }));
}

export async function closeTask(id) {
  await request("POST", `/tasks/${encodeURIComponent(id)}/close`);
}

export async function reopenTask(id) {
  await request("POST", `/tasks/${encodeURIComponent(id)}/reopen`);
}

export async function updateTask(id, changes) {
  return task(await request("POST", `/tasks/${encodeURIComponent(id)}`, { body: changes }));
}

export async function deleteTask(id) {
  await request("DELETE", `/tasks/${encodeURIComponent(id)}`);
}

export async function createTask(fields) {
  return task(await request("POST", "/tasks", { body: fields }));
}

/** Quick Add: Todoist parses dates, #projects, @labels and p1–p4 out of `text`. */
export async function quickAdd(text) {
  return task(await request("POST", "/tasks/quick", { body: { text } }));
}

export function taskUrl(id) {
  return `https://app.todoist.com/app/task/${encodeURIComponent(id)}`;
}

export function projectUrl(id) {
  return `https://app.todoist.com/app/project/${encodeURIComponent(id)}`;
}

/** Todoist's priority 4 is what its apps call P1. */
export const PRIORITIES = [
  { value: 4, title: "Priority 1", short: "P1", tone: "danger" },
  { value: 3, title: "Priority 2", short: "P2", tone: "warning" },
  { value: 2, title: "Priority 3", short: "P3", tone: "accent" },
  { value: 1, title: "Priority 4", short: "P4", tone: null },
];

export function priorityOf(value) {
  return PRIORITIES.find((priority) => priority.value === value) ?? PRIORITIES[3];
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

function pad(value) {
  return String(value).padStart(2, "0");
}

export function isoDay(date) {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** `overdue`, `today`, `upcoming` or `none` for a task's due date, in local time. */
export function dueBucket(task, now = new Date()) {
  if (!task.due?.date) return "none";
  const day = task.due.date.slice(0, 10);
  const today = isoDay(now);
  if (day < today) return "overdue";
  if (day > today) return "upcoming";
  if (task.due.date.length > 10) {
    const at = new Date(task.due.date);
    if (!Number.isNaN(at.getTime()) && at < now) return "overdue";
  }
  return "today";
}

/** A due date for people: `Today 09:00`, `Tomorrow`, `Mon 6 Oct`. */
export function dueText(task, now = new Date()) {
  if (!task.due?.date) return "";
  const date = task.due.date;
  const day = date.slice(0, 10);
  const today = isoDay(now);
  const tomorrow = isoDay(new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1));
  const yesterday = isoDay(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1));
  const [year, month, dom] = day.split("-").map(Number);
  const local = new Date(year, month - 1, dom);
  const name =
    day === today ? "Today"
    : day === tomorrow ? "Tomorrow"
    : day === yesterday ? "Yesterday"
    : `${WEEKDAYS[local.getDay()]} ${dom} ${MONTHS[month - 1]}${year !== now.getFullYear() ? ` ${year}` : ""}`;
  if (date.length <= 10) return name;
  const at = new Date(date);
  return Number.isNaN(at.getTime()) ? name : `${name} ${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/** The order tasks show in: by date and time, then priority, then Todoist's own order. */
export function compareTasks(a, b) {
  const da = a.due?.date ?? "9999";
  const db = b.due?.date ?? "9999";
  if (da !== db) return da < db ? -1 : 1;
  if (a.priority !== b.priority) return b.priority - a.priority;
  return a.order - b.order;
}
