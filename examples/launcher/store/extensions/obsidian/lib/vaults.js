// Finding Obsidian vaults and the notes inside them. Obsidian lists the
// vaults it knows in `obsidian.json`, in its settings folder; the
// "Extra Vault Folders" preference adds more. Every file call is made inside
// an async function, so a missing grant or file rejects instead of throwing
// out of `init` or `render`.
import { exists, mkdir, readFile, readdir, writeFile } from "fs/promises";
import { environment, launch } from "launcher/api";

const MAX_NOTES = 5000;
const MAX_DEPTH = 24;

/** Joins path parts with the separator `base` already uses. */
export function joinPath(base, ...parts) {
  const separator = base.includes("\\") && !base.includes("/") ? "\\" : "/";
  const clean = parts
    .flatMap((part) => String(part).split(/[\\/]+/))
    .filter((part) => part !== "" && part !== ".");
  const trimmed = base.replace(/[\\/]+$/, "");
  return [trimmed, ...clean].join(separator);
}

export function baseName(path) {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function expandHome(path) {
  const home = environment().home_path;
  return path.startsWith("~") ? joinPath(home, path.slice(1)) : path;
}

/** Where `obsidian.json` may be: the usual settings folder, then Flatpak and Snap on Linux. */
function settingsFiles() {
  const { config_path: config, home_path: home, platform } = environment();
  const files = [joinPath(config, "obsidian", "obsidian.json")];
  if (platform === "linux") {
    files.push(joinPath(home, ".var/app/md.obsidian.Obsidian/config/obsidian/obsidian.json"));
    files.push(joinPath(home, "snap/obsidian/current/.config/obsidian/obsidian.json"));
  }
  return files;
}

function extraFolders() {
  const extra = String(launch().preferences.extraVaults ?? "");
  return extra
    .split(/[;\n]+/)
    .map((path) => path.trim())
    .filter(Boolean)
    .map(expandHome);
}

/**
 * The vaults, open ones first, then the most recently opened. Each is
 * `{ id, name, path, open, ts }`; `name` is what `obsidian://` URIs call it.
 */
export async function loadVaults() {
  const vaults = [];
  for (const file of settingsFiles()) {
    try {
      if (!(await exists(file))) continue;
      const settings = JSON.parse(await readFile(file, "utf8"));
      for (const [id, vault] of Object.entries(settings.vaults ?? {})) {
        if (vault?.path) {
          vaults.push({ id, name: baseName(vault.path), path: vault.path, open: Boolean(vault.open), ts: vault.ts ?? 0 });
        }
      }
      break;
    } catch (_) {
      // An unreadable settings file: try the next place, then the extra folders.
    }
  }
  for (const path of extraFolders()) {
    if (!vaults.some((vault) => samePath(vault.path, path))) {
      vaults.push({ id: path, name: baseName(path), path, open: false, ts: 0 });
    }
  }
  return vaults.sort((a, b) => Number(b.open) - Number(a.open) || b.ts - a.ts);
}

function samePath(a, b) {
  const normal = (path) => path.replace(/[\\/]+/g, "/").replace(/\/$/, "").toLowerCase();
  return normal(a) === normal(b);
}

/** The vault the preference `name` names, or the open or most recent one. */
export function pickVault(vaults, name) {
  const wanted = String(name ?? "").trim().toLowerCase();
  if (wanted) {
    const match = vaults.find((vault) => vault.name.toLowerCase() === wanted || samePath(vault.path, wanted));
    if (match) return match;
  }
  return vaults[0] ?? null;
}

/**
 * Every Markdown note in `vault`, as `{ id, title, folder, relative, path, vault }`,
 * skipping hidden folders (`.obsidian`, `.trash`, `.git`) and `node_modules`.
 */
export async function listNotes(vault, limit = MAX_NOTES) {
  const notes = [];
  async function walk(directory, segments) {
    if (notes.length >= limit || segments.length > MAX_DEPTH) return;
    let entries;
    try {
      entries = await readdir(directory, { withFileTypes: true });
    } catch (_) {
      return;
    }
    const folders = [];
    for (const entry of entries) {
      const name = entry.name;
      if (name.startsWith(".") || name === "node_modules") continue;
      if (entry.isDirectory()) {
        folders.push(name);
      } else if (name.toLowerCase().endsWith(".md") && notes.length < limit) {
        const relative = [...segments, name].join("/");
        notes.push({
          id: `${vault.path}::${relative}`,
          title: name.slice(0, -3),
          folder: segments.join("/"),
          relative,
          path: joinPath(vault.path, ...segments, name),
          vault,
        });
      }
    }
    for (const folder of folders) {
      await walk(joinPath(directory, folder), [...segments, folder]);
    }
  }
  await walk(vault.path, []);
  return notes;
}

/** `obsidian://open` for a vault, or for a note in it when `relative` is given. */
export function obsidianUri(vault, relative) {
  const base = `obsidian://open?vault=${encodeURIComponent(vault.name)}`;
  if (!relative) return base;
  return `${base}&file=${encodeURIComponent(relative.replace(/\.md$/i, ""))}`;
}

export async function readNote(path) {
  return readFile(path, "utf8");
}

/** Characters no file system or Obsidian link accepts in a note's name. */
export function safeFileName(title) {
  return title.replace(/[\\/:*?"<>|#^[\]]/g, " ").replace(/\s+/g, " ").trim();
}

/** Creates `folder` inside the vault and a note named after `title` in it, never overwriting. */
export async function createNote(vault, folder, title, content) {
  const name = safeFileName(title) || "Untitled";
  const segments = String(folder ?? "")
    .split(/[\\/]+/)
    .map((segment) => safeFileName(segment))
    .filter(Boolean);
  const directory = joinPath(vault.path, ...segments);
  if (segments.length > 0) await mkdir(directory, { recursive: true });
  let fileName = `${name}.md`;
  for (let copy = 1; await exists(joinPath(directory, fileName)); copy += 1) {
    fileName = `${name} ${copy}.md`;
  }
  const path = joinPath(directory, fileName);
  await writeFile(path, content);
  return { path, relative: [...segments, fileName].join("/") };
}

/** Appends `text` as its own line(s) to the note at `relative`, creating it and its folders. */
export async function appendToNote(vault, relative, text) {
  const segments = relative.split(/[\\/]+/).filter(Boolean);
  const path = joinPath(vault.path, ...segments);
  if (segments.length > 1) await mkdir(joinPath(vault.path, ...segments.slice(0, -1)), { recursive: true });
  let current = "";
  if (await exists(path)) current = await readFile(path, "utf8");
  const separator = current === "" || current.endsWith("\n") ? "" : "\n";
  await writeFile(path, `${current}${separator}${text}\n`);
  return path;
}

const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/** Formats `date` with the Moment.js tokens Daily Notes formats commonly use. */
export function formatMoment(date, format) {
  const pad = (value) => String(value).padStart(2, "0");
  const tokens = {
    YYYY: () => String(date.getFullYear()),
    YY: () => String(date.getFullYear()).slice(-2),
    MMMM: () => MONTHS[date.getMonth()],
    MMM: () => MONTHS[date.getMonth()].slice(0, 3),
    MM: () => pad(date.getMonth() + 1),
    M: () => String(date.getMonth() + 1),
    DD: () => pad(date.getDate()),
    Do: () => ordinal(date.getDate()),
    D: () => String(date.getDate()),
    dddd: () => DAYS[date.getDay()],
    ddd: () => DAYS[date.getDay()].slice(0, 3),
    HH: () => pad(date.getHours()),
    mm: () => pad(date.getMinutes()),
  };
  return format.replace(/\[([^\]]*)\]|YYYY|YY|MMMM|MMM|MM|M|Do|DD|D|dddd|ddd|HH|mm/g, (match, literal) =>
    literal !== undefined ? literal : tokens[match](),
  );
}

function ordinal(day) {
  const suffix = day % 10 === 1 && day !== 11 ? "st" : day % 10 === 2 && day !== 12 ? "nd" : day % 10 === 3 && day !== 13 ? "rd" : "th";
  return `${day}${suffix}`;
}

/** Today's daily note in `vault`, as a path relative to it, from the Daily Notes core plugin's settings. */
export async function dailyNotePath(vault, date = new Date()) {
  let settings = {};
  try {
    const file = joinPath(vault.path, ".obsidian", "daily-notes.json");
    if (await exists(file)) settings = JSON.parse(await readFile(file, "utf8"));
  } catch (_) {
    settings = {};
  }
  const name = formatMoment(date, settings.format || "YYYY-MM-DD");
  const folder = String(settings.folder ?? "").replace(/^[\\/]+|[\\/]+$/g, "");
  return folder ? `${folder}/${name}.md` : `${name}.md`;
}

export function timestamp(date = new Date()) {
  return formatMoment(date, "YYYY-MM-DD HH:mm");
}

/** Says why no vault was found. */
export const NO_VAULTS = {
  title: "No Obsidian vaults found",
  description: "Open a vault in Obsidian once, or add its folder under the extension's “Extra Vault Folders” preference.",
};
