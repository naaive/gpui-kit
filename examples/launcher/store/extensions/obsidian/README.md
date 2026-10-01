# Obsidian

Search, create and capture notes in your [Obsidian](https://obsidian.md) vaults
without opening Obsidian first.

## Commands

- **Search Obsidian Notes** — every Markdown note across your vaults, matched by
  title and folder and, once they are read in the background, by content. The
  note's text shows beside the list. Open it in Obsidian or in the default app,
  reveal it, or copy a Markdown link, the `obsidian://` URI, a wiki link or the
  path.
- **Create Obsidian Note** — a form with the vault, a folder inside it, the
  title and the text. It writes a new `.md` file (never overwriting one) and
  offers to open it in Obsidian. Type a title after the command to fill it in.
- **Quick Capture to Obsidian** — type text after the command; it is appended
  with a timestamp to your inbox note or to today's daily note (the Daily Notes
  core plugin's folder and date format are honoured), and a HUD confirms it.
- **Search Obsidian Vaults** — open a vault in Obsidian, search only its notes,
  or show its folder.

## Setup

Nothing to set up: vaults are read from Obsidian's own list,
`obsidian.json` in its settings folder (`%APPDATA%\obsidian` on Windows,
`~/Library/Application Support/obsidian` on macOS, `~/.config/obsidian` on
Linux, plus the Flatpak and Snap locations). Add vaults Obsidian does not list
under **Extra Vault Folders**, and choose the vault Create Note and Quick
Capture use under **Default Vault**.

## Permissions

- **Read `${configDir}/obsidian`** — Obsidian's list of vaults.
- **Read `${homeDir}`** — your notes. A manifest's file grants are fixed
  before the extension runs, so it cannot ask for exactly the vault folders
  Obsidian lists; your home folder is the narrowest grant that covers vaults
  wherever you keep them under it. Vaults outside your home folder (another
  drive, say) cannot be read.
- **Write `${homeDir}`** — Create Note writes the new note, and Quick Capture
  appends to the inbox or daily note, inside the vault. Nothing else is
  written.

No network access is requested.
