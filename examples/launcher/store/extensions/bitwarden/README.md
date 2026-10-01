# Bitwarden

Search your [Bitwarden](https://bitwarden.com) vault, copy or paste
passwords and TOTP codes, and generate passwords, through the official
Bitwarden CLI (`bw`).

## Commands

- **Search Bitwarden Vault** — checks `bw status`; a locked vault asks for
  your master password and unlocks it with `bw unlock --raw`. Logins, cards,
  secure notes and identities are listed with their type, username and
  websites. Copy Password, Copy Username, Copy TOTP, Paste Password and Open
  URL for logins; Copy Card Number and Copy Security Code for cards; Copy Notes;
  Sync Vault and Lock Vault.
- **Generate Password** — `bw generate` options (length and character sets,
  or a passphrase's words and separator); submit to copy or paste the result.
  The options are remembered, never the password.
- **Lock Bitwarden Vault** — runs `bw lock`, which also invalidates every
  session key.

## Setup

1. Install the CLI: `winget install Bitwarden.CLI` (Windows),
   `brew install bitwarden-cli` (macOS), `snap install bw` (Linux), or
   `npm install -g @bitwarden/cli`. `bw` must be on your `PATH`.
2. Log in once in a terminal: `bw login` (for a self-hosted server, run
   `bw config server https://your.server` first).
3. Make `bw` portable. The launcher starts programs **without environment
   variables**, so `bw` cannot find the data folder `bw login` wrote
   (`%APPDATA%\Bitwarden CLI`, `~/.config/Bitwarden CLI`) and reports itself
   logged out. Create a folder named `bw-data` next to the `bw` executable
   (next to `node.exe` / `node` for an npm install) — bw then keeps its data
   there whatever the environment — and run `bw login` once more in a
   terminal.

## How secrets are handled

- The session key is kept in memory by the Search page only, and is forgotten
  when the page closes; it is never written to `localStorage` or the cache.
  Opening the command again asks for the master password again.
- Rows and the detail pane show names, usernames, websites and whether a
  password or TOTP exists, never a secret. A password, TOTP, card number or
  note is fetched from `bw` only when you copy or paste it.
- A copied secret is cleared from the clipboard after 30 seconds if the
  clipboard still holds it and the Search page is still open.
- Nothing `bw` prints is logged.

Known limits: the master password is passed to `bw unlock` as an argument (the
launcher cannot give a program standard input or environment variables), so
it is briefly visible to other programs that list process command lines. A
copied secret may be recorded by clipboard history (the launcher's, or the
system's) because the launcher's copy cannot mark it private.

## Permissions

- **Run `bw`** — the only program it runs.
- **Clipboard read and write** — to clear a copied secret after 30 seconds,
  only if the clipboard still holds it.

No network or file access is requested; `bw` talks to Bitwarden itself.
