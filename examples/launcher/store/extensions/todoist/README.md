# Todoist

See, complete, reschedule and create [Todoist](https://todoist.com) tasks,
with the number due today in the tray.

## Commands

- **Search Todoist Tasks** — your active tasks: overdue and today's first, then
  each of the next seven days, later ones, and those without a date; or grouped
  by project (the dropdown beside the search field). Priority shows as a
  colored tag. Complete Task (removed at once, with Undo on the toast), Open in
  Todoist, Change Priority, Reschedule (Today, Tomorrow, Next Week, Pick Date…,
  Remove Due Date), Copy Task, Delete Task… (asks first). The last list shows
  at once from the cache while a fresh one loads.
- **Create Todoist Task** — a form with the task, description, project, a due
  date in natural language (“tomorrow 9am”, “every friday”) and priority. Type
  the task after the command to add it at once with Todoist's Quick Add, which
  also understands `#Project`, `@label` and `p1` in the text.
- **Todoist Today in Tray** — a tray icon with the number of tasks due today
  (overdue included), refreshed every 10 minutes; choose a task in its menu to
  complete it.

## Setup

Copy your personal API token from Todoist **Settings → Integrations →
Developer** and paste it into the extension's **API Token** preference. It is
kept in the system keychain.

The extension uses Todoist's [API v1](https://developer.todoist.com/api/v1/);
the older REST v2 now answers `410 Gone`.

## Permissions

HTTPS to `api.todoist.com` only:

- `GET /api/v1/tasks`, `/api/v1/tasks/filter`, `/api/v1/projects` — list tasks
  and projects.
- `POST /api/v1/tasks…` — create, quick-add, update, close and reopen tasks.
- `DELETE /api/v1/tasks/…` — delete a task.
