# Extension Store

What the launcher's Extension Store lists. Each folder in `extensions/` is an
extension, written against the SDK described in the launcher's README;
`index.json` lists them with a SHA-256 for every file, and the launcher reads
both from GitHub (`raw.githubusercontent.com`), so publishing an extension is
committing it here.

After adding or changing an extension, write the index again:

```text
launcher lint store/extensions/<name>
launcher store-index store
```

An extension's `launcher.json` gives the store its `description`, `author`
and `categories`, and its `gpui-shell.json` its `version`: raise the version
for users to be offered the update.

## Extensions

| Extension | Commands | Does |
| --- | --- | --- |
| [Bitwarden](extensions/bitwarden) | 3 | Search your Bitwarden vault, copy passwords and TOTP codes, and generate passwords with the Bitwarden CLI. |
| [Developer Tools](extensions/dev-tools) | 9 | Format JSON, encode and decode text, hash, generate UUIDs and passwords, convert timestamps and test regular expressions, all offline. |
| [Docker](extensions/docker) | 2 | Start, stop, inspect and remove Docker containers and images. |
| [Linear](extensions/linear) | 3 | See, search, create and update your Linear issues. |
| [Network Tools](extensions/network-tools) | 3 | See your public and local IP addresses, find which process listens on a port, and look up DNS records. |
| [Notion](extensions/notion) | 3 | Search your Notion pages and databases, and write new pages and notes. |
| [Obsidian](extensions/obsidian) | 4 | Search, create and capture notes in your Obsidian vaults. |
| [Package Search](extensions/package-search) | 4 | Search npm, crates.io, PyPI and MDN as you type, and copy install commands. |
| [Recent Projects](extensions/recent-projects) | 1 | Reopen folders and workspaces from VS Code, Cursor, VSCodium, Windsurf and JetBrains IDEs. |
| [Spotify](extensions/spotify) | 7 | Search Spotify, see what is playing and control playback from the launcher and the tray. |
| [Todoist](extensions/todoist) | 3 | See, complete, reschedule and create Todoist tasks, with today's count in the tray. |
| [Web Search](extensions/web-search) | 1 | Search Google, Bing, DuckDuckGo, Wikipedia or YouTube with suggestions as you type. |
