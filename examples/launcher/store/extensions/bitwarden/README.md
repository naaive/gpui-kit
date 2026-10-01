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

## How secrets are handled

- The session key is kept in memory by the Search page only, and is forgotten
  when the page closes; it is never written to `localStorage` or the cache.
  Opening the command again asks for the master password again.
- Rows and the detail pane show names, usernames, websites and whether a
  password or TOTP exists, never a secret. A password, TOTP, card number or
  note is fetched from `bw` only when you copy or paste it.
- Secrets are copied and pasted concealed: the launcher's Clipboard History
  leaves them out, and the launcher clears the clipboard after 30 seconds if
  it still holds the secret, even after the page has closed.
- Nothing `bw` prints is logged.

The master password reaches `bw unlock` through an environment variable
(`--passwordenv`), never as an argument, so other programs listing command
lines do not see it. Clipboard managers other than the launcher's may still
record a copied secret.

## Permissions

- **Run `bw`** — the only program it runs.

No network or file access is requested; `bw` talks to Bitwarden itself.
