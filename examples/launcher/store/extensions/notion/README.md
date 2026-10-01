# Notion

Search your Notion workspace and write to it from the launcher.

## Commands

- **Search Notion**: pages and databases as you type (recently edited ones
  first), with icons, last-edited dates, the page's properties and its first
  blocks beside the list. Actions: Open in Notion (the desktop app), Open in
  Browser, Copy URL, Copy Markdown Link, Create Page Inside…, Append to Page….
- **Create Notion Page**: a quick note: choose a parent page or database, give
  a title and text; each line becomes a paragraph.
- **Append to Notion Page**: add text at the end of a page, as paragraphs or
  as to-dos.

## Setup

1. Open [notion.so/my-integrations](https://www.notion.so/my-integrations),
   create an **internal** integration for your workspace, and give it the
   *Read content*, *Insert content* and *Update content* capabilities.
2. Copy its **Internal Integration Secret** into the extension's
   **Integration Token** preference (it is kept in the system keychain).
3. An integration sees only what is shared with it: in Notion, open each
   top-level page you want here, choose **••• → Connections**, and add the
   integration. Pages below a shared page are included.

## Permissions

- Network, `api.notion.com` only: `POST /v1/search` and `/v1/pages`, and
  `GET` and `PATCH` under `/v1/blocks/` (a page's content, and appending to
  it). Requests use the `2022-06-28` API version.
