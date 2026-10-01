// Small path helpers that agree with the platform the launcher runs on,
// without depending on how the runtime's `path` module spells separators.
import { environment } from "launcher/api";

export function platform() {
  try {
    return environment().platform;
  } catch {
    return "linux";
  }
}

export function isWindows() {
  return platform() === "windows";
}

/** Joins path parts with the platform's separator, dropping duplicate ones. */
export function join(...parts) {
  const separator = isWindows() ? "\\" : "/";
  return parts
    .filter((part) => part != null && part !== "")
    .map((part, ix) => {
      const text = String(part);
      const trimmed = ix === 0 ? text.replace(/[\\/]+$/, "") : text.replace(/^[\\/]+|[\\/]+$/g, "");
      return isWindows() ? trimmed.replace(/\//g, "\\") : trimmed;
    })
    .join(separator);
}

/** The last part of a path or URI, without a trailing separator. */
export function basename(path) {
  const parts = String(path).replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || String(path);
}

/** A local path in the platform's spelling: `C:/x/y` → `C:\x\y` on Windows. */
export function native(path) {
  return isWindows() ? String(path).replace(/\//g, "\\") : String(path);
}

/** A path with the home directory shown as `~`, for a subtitle. */
export function tildify(path, home) {
  if (!home) return path;
  const head = path.slice(0, home.length);
  const same = isWindows() ? head.toLowerCase() === home.toLowerCase() : head === home;
  const rest = path.slice(home.length);
  return same && (rest === "" || /^[\\/]/.test(rest)) ? `~${rest}` : path;
}
