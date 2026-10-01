# GitHub

Search GitHub, and follow your pull requests, issues and notifications.

## Commands

- **Search GitHub Repositories**: searches as you type; also offered as a fallback for any search text.
- **My Pull Requests**: open pull requests you created, were asked to review, or are assigned, with draft and CI status.
  Actions: Open in Browser, Copy URL, Copy Branch Name, Copy Checkout Command (`gh pr checkout N --repo owner/name`).
- **My Issues**: open issues assigned to you, created by you, or mentioning you.
- **GitHub Notifications**: unread notifications; Open in Browser, Mark as Read, Mark All as Read.
- **Unread GitHub Notifications in Tray**: a tray icon with the unread count and the latest notifications, refreshed every 5 minutes.

## Setup

Search works without a token. Everything else needs a personal access token: create a classic token at
<https://github.com/settings/tokens/new?scopes=repo,notifications> with the `repo` and `notifications` scopes
(fine-grained tokens cannot read notifications). Then select any GitHub command in the root search, choose
Configure Extension (Ctrl+Shift+, or ⌘⇧,), and paste it into Personal Access Token. The token is kept in the
system keychain.

## Permissions

- `GET https://api.github.com/search/repositories`: repository search.
- `POST https://api.github.com/graphql`: pull requests and issues, one query each.
- `GET https://api.github.com/notifications`: unread notifications.
- `PATCH https://api.github.com/notifications/threads/*`: Mark as Read.
- `PUT https://api.github.com/notifications`: Mark All as Read.
