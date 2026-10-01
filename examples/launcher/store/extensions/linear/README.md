# Linear

Your Linear issues in the launcher: see what is assigned to you, search the
workspace, change status and priority, and create issues.

## Commands

- **My Issues**: open issues assigned to you, grouped by state (In Progress,
  Todo, Backlog…) and ordered by priority, with the description beside the
  list. Actions: Open in Linear, Copy Git Branch Name, Copy Issue ID, Copy URL,
  Change Status (the team's workflow states), Change Priority, Assign to Me.
- **Search Issues**: full-text search over every issue as you type, with the
  same detail and actions.
- **Create Issue**: a form with team, title, description (Markdown), priority
  and "Assign to me"; the toast's **Open** button opens the new issue.

## Setup

In Linear, open **Settings → Security & access → Personal API keys**, create a
key, and paste it into the extension's **Personal API Key** preference. The
key acts as you, with your access to teams and issues; it is kept in the
system keychain.

## Permissions

- Network: `POST https://api.linear.app/graphql`, nothing else.
