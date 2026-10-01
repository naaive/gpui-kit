// Runs the Bitwarden CLI (`bw`). The manifest allows this one program and
// nothing else. A session key is passed in by the caller, which keeps it in
// a field for as long as its page is open; it is never written anywhere.
// Nothing here logs what `bw` prints, since that may be a secret.
import { run } from "process";
import { environment } from "launcher/api";

export const ITEM_TYPES = {
  1: { title: "Login", icon: "key-round" },
  2: { title: "Secure Note", icon: "sticky-note" },
  3: { title: "Card", icon: "credit-card" },
  4: { title: "Identity", icon: "id-card" },
  5: { title: "SSH Key", icon: "key" },
};

/** A failure with a title and a hint fit for an empty state. */
export class BwError extends Error {
  constructor(title, hint) {
    super(hint);
    this.title = title;
    this.hint = hint;
  }
}

function installHint() {
  switch (environment().platform) {
    case "windows":
      return "Install the Bitwarden CLI with “winget install Bitwarden.CLI” (or Scoop, Chocolatey, or npm i -g @bitwarden/cli), make sure “bw” is on your PATH, then reopen the launcher.";
    case "macos":
      return "Install the Bitwarden CLI with “brew install bitwarden-cli” (or npm i -g @bitwarden/cli), then reopen the launcher.";
    default:
      return "Install the Bitwarden CLI with “snap install bw” (or npm i -g @bitwarden/cli), make sure “bw” is on your PATH, then reopen the launcher.";
  }
}

/**
 * Runs `bw` with `args`, the session if any, and never an interactive prompt;
 * `operands` follow `--`, so a value starting with "-" is not read as an
 * option. Answers stdout.
 */
export async function bw(args, session = null, operands = [], env = {}) {
  const full = [
    ...args,
    "--nointeraction",
    ...(session ? ["--session", session] : []),
    ...(operands.length > 0 ? ["--", ...operands] : []),
  ];
  let output;
  try {
    output = await run("bw", full, { env });
  } catch (error) {
    const message = String(error?.message ?? error);
    if (/not found|not granted/i.test(message)) throw new BwError("Bitwarden CLI not found", installHint());
    throw new BwError("Cannot run the Bitwarden CLI", message);
  }
  if (output.code !== 0) {
    const said = (output.stderr || output.stdout || "").trim();
    if (/invalid master password/i.test(said)) throw new BwError("Invalid master password", "Check the password and try again.");
    if (/not logged in|you are not logged in/i.test(said)) throw new BwError("Not logged in", loginHint());
    if (/vault is locked/i.test(said)) throw new BwError("Vault is locked", "Unlock it again.");
    // The first line only: an error message never carries a secret, but keep it short.
    throw new BwError("Bitwarden CLI failed", said.split("\n")[0] || `bw exited with ${output.code}`);
  }
  return output.stdout;
}

export function loginHint() {
  return "Run “bw login” in a terminal (after “bw config server <url>” for a self-hosted server), then reopen this command.";
}

/** `{ status: "unauthenticated" | "locked" | "unlocked", userEmail, serverUrl, lastSync }`. */
export async function status(session = null) {
  const text = await bw(["status"], session);
  try {
    return JSON.parse(text);
  } catch (_) {
    throw new BwError("Unexpected answer from bw status", "Update the Bitwarden CLI and try again.");
  }
}

/** Unlocks the vault and answers the session key. */
export async function unlock(password) {
  // From a variable bw is told to read, not an argument: other programs can
  // list a process's arguments.
  const key = (await bw(["unlock", "--raw", "--passwordenv", "BW_PASSWORD"], null, [], { BW_PASSWORD: password })).trim();
  if (!key) throw new BwError("Cannot unlock the vault", "bw answered no session key.");
  return key;
}

/**
 * The vault's items without their secrets: the list keeps a username, the
 * item's addresses and whether it has a password or a TOTP, and asks `bw`
 * for a secret only when an action needs it.
 */
export async function listItems(session) {
  const items = JSON.parse(await bw(["list", "items"], session));
  return items.map((item) => ({
    id: item.id,
    name: item.name ?? "Untitled",
    type: item.type,
    favorite: Boolean(item.favorite),
    folder_id: item.folderId ?? null,
    organization: Boolean(item.organizationId),
    revision: item.revisionDate ?? null,
    username: item.login?.username ?? null,
    uris: (item.login?.uris ?? []).map((entry) => entry.uri).filter(Boolean),
    has_password: Boolean(item.login?.password),
    has_totp: Boolean(item.login?.totp),
    has_notes: Boolean(item.notes),
    card_brand: item.card?.brand ?? null,
    card_expiry: item.card?.expMonth && item.card?.expYear ? `${item.card.expMonth}/${item.card.expYear}` : null,
    has_card_number: Boolean(item.card?.number),
    has_card_code: Boolean(item.card?.code),
    identity_name: [item.identity?.firstName, item.identity?.lastName].filter(Boolean).join(" ") || null,
  }));
}

export async function listFolders(session) {
  return JSON.parse(await bw(["list", "folders"], session));
}

/** One field of an item: `password`, `username`, `totp` or `notes`. */
export async function getField(field, id, session) {
  return (await bw(["get", field, id], session)).replace(/\r?\n$/, "");
}

/** A field of an item's card: `number` or `code`. */
export async function getCardField(field, id, session) {
  const item = JSON.parse(await bw(["get", "item", id], session));
  return item.card?.[field] ?? "";
}

export async function sync(session) {
  await bw(["sync"], session);
}

export async function lock() {
  await bw(["lock"]);
}

/** A password or passphrase from `bw generate`, with `options` from the Generate form. */
export async function generate(options) {
  const args = ["generate"];
  if (options.kind === "passphrase") {
    args.push("--passphrase", "--words", String(options.words), "--separator", options.separator || "-");
    if (options.capitalize) args.push("--capitalize");
    if (options.include_number) args.push("--includeNumber");
  } else {
    if (options.uppercase) args.push("--uppercase");
    if (options.lowercase) args.push("--lowercase");
    if (options.number) args.push("--number");
    if (options.special) args.push("--special");
    args.push("--length", String(options.length));
  }
  return (await bw(args)).trim();
}

/** A URL `open_url` accepts: Bitwarden stores many without a scheme. */
export function openableUrl(uri) {
  return /^[a-z][a-z0-9+.-]*:/i.test(uri) ? uri : `https://${uri}`;
}

export function hostOf(uri) {
  const match = /^(?:[a-z][a-z0-9+.-]*:\/\/)?([^/?#:]+)/i.exec(uri);
  return match ? match[1] : uri;
}
