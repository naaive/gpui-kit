// Recent folders, workspaces and files of the editors built on VS Code. Each
// keeps them in `<configDir>/<Product>/User/globalStorage/state.vscdb`, an
// SQLite database whose `ItemTable` holds a JSON list under
// `history.recentlyOpenedPathsList`, newest first.
import { exists } from "fs/promises";
import { environment, sql_query } from "launcher/api";
import { basename, isWindows, join } from "./paths.js";

const RECENT_KEY = "history.recentlyOpenedPathsList";

export const CODE_EDITORS = [
  {
    id: "vscode",
    title: "Visual Studio Code",
    folder: "Code",
    scheme: "vscode",
    icon: "code",
    windows: "Microsoft VS Code/Code.exe",
    mac: "Visual Studio Code",
    linux: "code",
  },
  {
    id: "vscode-insiders",
    title: "VS Code Insiders",
    folder: "Code - Insiders",
    scheme: "vscode-insiders",
    icon: "code-xml",
    windows: "Microsoft VS Code Insiders/Code - Insiders.exe",
    mac: "Visual Studio Code - Insiders",
    linux: "code-insiders",
  },
  {
    id: "cursor",
    title: "Cursor",
    folder: "Cursor",
    scheme: "cursor",
    icon: "mouse-pointer-2",
    windows: "cursor/Cursor.exe",
    mac: "Cursor",
    linux: "cursor",
  },
  {
    id: "vscodium",
    title: "VSCodium",
    folder: "VSCodium",
    scheme: "vscodium",
    icon: "square-code",
    windows: "VSCodium/VSCodium.exe",
    mac: "VSCodium",
    linux: "codium",
  },
  {
    id: "windsurf",
    title: "Windsurf",
    folder: "Windsurf",
    scheme: "windsurf",
    icon: "wind",
    windows: "Windsurf/Windsurf.exe",
    mac: "Windsurf",
    linux: "windsurf",
  },
];

/**
 * A VS Code URI as a project location. `file:///c%3A/x` is local (`C:\x` on
 * Windows); `vscode-remote://wsl%2Bubuntu/home/x` is remote, with its
 * authority (`wsl+ubuntu`) and the path on that machine.
 */
export function parseUri(uri) {
  const match = /^([a-z][\w+.-]*):\/\/([^/]*)(\/.*)?$/i.exec(String(uri ?? ""));
  if (!match) return null;
  const [, scheme, rawAuthority, rawPath = "/"] = match;
  let path;
  let authority;
  try {
    path = decodeURIComponent(rawPath);
    authority = decodeURIComponent(rawAuthority);
  } catch {
    return null;
  }
  if (scheme.toLowerCase() === "file") {
    if (isWindows()) {
      const drive = /^\/([a-zA-Z]):(\/.*)?$/.exec(path);
      const local = drive
        ? `${drive[1].toUpperCase()}:${(drive[2] ?? "\\").replace(/\//g, "\\")}`
        : authority
          ? `\\\\${authority}${path.replace(/\//g, "\\")}`
          : path.replace(/\//g, "\\");
      return { local: true, path: local };
    }
    return { local: true, path: authority ? `//${authority}${path}` : path };
  }
  if (scheme.toLowerCase() === "vscode-remote") {
    return { local: false, authority, path, rawPath };
  }
  return null;
}

/** `wsl+Ubuntu` → `WSL: Ubuntu`; `ssh-remote+host` → `SSH: host`. */
export function describeRemote(authority) {
  const [kind, ...rest] = authority.split("+");
  const target = rest.join("+");
  const names = { wsl: "WSL", "ssh-remote": "SSH", "dev-container": "Dev Container", "attached-container": "Container", tunnel: "Tunnel", codespaces: "Codespaces" };
  const name = names[kind] ?? kind;
  // Container targets are hex-encoded JSON; their name is not worth decoding here.
  return target && !/^[0-9a-f]{16,}$/i.test(target) ? `${name}: ${target}` : name;
}

function toProject(editor, entry, order) {
  let kind;
  let uri;
  if (entry.folderUri) {
    kind = "folder";
    uri = entry.folderUri;
  } else if (entry.workspace?.configPath) {
    kind = "workspace";
    uri = entry.workspace.configPath;
  } else if (entry.fileUri) {
    kind = "file";
    uri = entry.fileUri;
  } else {
    return null;
  }
  const location = parseUri(uri);
  if (!location) return null;
  const name = basename(location.path).replace(/\.code-workspace$/, "");
  const remote = location.local ? null : describeRemote(location.authority);
  // A remote WSL folder can still be reached from Windows through its share.
  let reveal = location.local ? location.path : null;
  if (!location.local && isWindows() && /^wsl\+/i.test(location.authority)) {
    reveal = `\\\\wsl.localhost\\${location.authority.slice(4)}${location.path.replace(/\//g, "\\")}`;
  }
  return {
    id: `${editor.id}:${uri}`,
    editor: editor.id,
    kind,
    name: kind === "workspace" ? `${name} (Workspace)` : name,
    path: location.path,
    uri,
    local: location.local,
    authority: location.local ? null : location.authority,
    remote,
    reveal,
    order,
    time: null,
  };
}

/** Answers this editor's recent projects, or `[]` when it is not installed. */
export async function readCodeEditor(editor) {
  const { config_path } = environment();
  const database = join(config_path, editor.folder, "User", "globalStorage", "state.vscdb");
  if (!(await exists(database))) return [];
  const rows = await sql_query(database, "SELECT value FROM ItemTable WHERE key = ?", [RECENT_KEY]);
  const value = rows[0]?.value;
  if (typeof value !== "string") return [];
  const entries = JSON.parse(value)?.entries ?? [];
  return entries.map((entry, order) => toProject(editor, entry, order)).filter(Boolean);
}

/** Where this editor's program is on Windows, if it is installed in a usual place. */
export async function findCodeExecutable(editor) {
  const { home_path } = environment();
  const roots = [join(home_path, "AppData", "Local", "Programs"), "C:\\Program Files"];
  for (const root of roots) {
    const candidate = join(root, editor.windows);
    try {
      if (await exists(candidate)) return candidate;
    } catch {
      // Not granted, or not there: try the next place.
    }
  }
  return null;
}

/** A URL the editor itself handles, for a project its program cannot be given directly. */
export function editorUrl(editor, project) {
  if (project.local) {
    const forward = project.path.replace(/\\/g, "/").replace(/^\/*/, "");
    return `${editor.scheme}://file/${encodeURI(forward)}`;
  }
  return `${editor.scheme}://vscode-remote/${project.authority}${encodeURI(project.path)}`;
}
