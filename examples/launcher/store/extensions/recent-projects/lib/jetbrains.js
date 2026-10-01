// Recent projects of JetBrains IDEs. Each IDE version keeps them in
// `<configDir>/JetBrains/<Product><version>/options/recentProjects.xml`:
// newer versions as `entry` keys of the `additionalInfo` map, with an
// `activationTimestamp`, older ones as a plain `recentPaths` list. The file is
// small and regular, so it is read with patterns rather than an XML parser.
import { exists, readdir, readFile } from "fs/promises";
import { environment } from "launcher/api";
import { basename, isWindows, join, native } from "./paths.js";

/** Config folder prefix → how the IDE is named, its launcher script, and its install folder. */
export const JETBRAINS_PRODUCTS = {
  IntelliJIdea: { title: "IntelliJ IDEA", script: "idea", install: "IntelliJ IDEA", icon: "brain-circuit" },
  IdeaIC: { title: "IntelliJ IDEA CE", script: "idea", install: "IntelliJ IDEA Community Edition", icon: "brain-circuit" },
  PyCharm: { title: "PyCharm", script: "pycharm", install: "PyCharm", icon: "flask-conical" },
  PyCharmCE: { title: "PyCharm CE", script: "pycharm", install: "PyCharm Community Edition", icon: "flask-conical" },
  WebStorm: { title: "WebStorm", script: "webstorm", install: "WebStorm", icon: "globe" },
  PhpStorm: { title: "PhpStorm", script: "phpstorm", install: "PhpStorm", icon: "file-code" },
  CLion: { title: "CLion", script: "clion", install: "CLion", icon: "cpu" },
  GoLand: { title: "GoLand", script: "goland", install: "GoLand", icon: "rabbit" },
  Rider: { title: "Rider", script: "rider", install: "JetBrains Rider", icon: "hash" },
  RustRover: { title: "RustRover", script: "rustrover", install: "RustRover", icon: "cpu" },
  RubyMine: { title: "RubyMine", script: "rubymine", install: "RubyMine", icon: "file-code" },
  DataGrip: { title: "DataGrip", script: "datagrip", install: "DataGrip", icon: "hard-drive" },
  DataSpell: { title: "DataSpell", script: "dataspell", install: "DataSpell", icon: "flask-conical" },
  Aqua: { title: "Aqua", script: "aqua", install: "Aqua", icon: "bug" },
};

const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };

function unescapeXml(text) {
  return text.replace(/&(#x[0-9a-f]+|#\d+|\w+);/gi, (whole, name) => {
    if (name[0] === "#") {
      const code = name[1].toLowerCase() === "x" ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10);
      return Number.isFinite(code) ? String.fromCodePoint(code) : whole;
    }
    return ENTITIES[name] ?? whole;
  });
}

function expand(raw, home) {
  return native(unescapeXml(raw).replace(/\$USER_HOME\$/g, home.replace(/\\/g, "/")));
}

/** `[{ path, time }]` from one `recentProjects.xml`, newest first. */
export function parseRecentProjects(xml, home) {
  const projects = [];
  const entries = /<entry key="([^"]+)">([\s\S]*?)<\/entry>/g;
  for (const [, key, body] of xml.matchAll(entries)) {
    if (!/RecentProjectMetaInfo/.test(body)) continue;
    const stamp = /name="(?:activationTimestamp|projectOpenTimestamp)" value="(\d+)"/.exec(body);
    projects.push({ path: expand(key, home), time: stamp ? Number(stamp[1]) : null });
  }
  if (projects.length === 0) {
    const list = /<option name="recentPaths">\s*<list>([\s\S]*?)<\/list>/.exec(xml);
    for (const [, value] of (list?.[1] ?? "").matchAll(/<option value="([^"]+)"/g)) {
      projects.push({ path: expand(value, home), time: null });
    }
  }
  return projects.sort((a, b) => (b.time ?? 0) - (a.time ?? 0));
}

/** `PyCharm2025.2` → `{ product: "PyCharm", version: "2025.2" }`. */
function parseFolder(name) {
  const match = /^([A-Za-z]+?)(\d{4}\.\d+)$/.exec(name);
  return match && JETBRAINS_PRODUCTS[match[1]] ? { product: match[1], version: match[2] } : null;
}

/**
 * Every IDE's recent projects, one list per product: versions of one IDE are
 * merged, and a project opened in several keeps its newest time.
 */
export async function readJetBrains() {
  const { config_path, home_path } = environment();
  const root = join(config_path, "JetBrains");
  if (!(await exists(root))) return [];
  const byProduct = new Map();
  for (const name of await readdir(root)) {
    const folder = parseFolder(name);
    if (!folder) continue;
    const file = join(root, name, "options", "recentProjects.xml");
    let xml;
    try {
      if (!(await exists(file))) continue;
      xml = await readFile(file, "utf8");
    } catch {
      continue;
    }
    const merged = byProduct.get(folder.product) ?? new Map();
    for (const project of parseRecentProjects(xml, home_path)) {
      const key = isWindows() ? project.path.toLowerCase() : project.path;
      const known = merged.get(key);
      if (!known || (project.time ?? 0) > (known.time ?? 0)) merged.set(key, project);
    }
    byProduct.set(folder.product, merged);
  }
  return [...byProduct].map(([product, merged]) => {
    const info = JETBRAINS_PRODUCTS[product];
    const editor = `jetbrains-${product.toLowerCase()}`;
    const projects = [...merged.values()]
      .sort((a, b) => (b.time ?? 0) - (a.time ?? 0))
      .map((project, order) => ({
        id: `${editor}:${project.path}`,
        editor,
        kind: "folder",
        name: basename(project.path),
        path: project.path,
        local: true,
        remote: null,
        reveal: project.path,
        order,
        time: project.time,
      }));
    return { editor: { id: editor, product, title: info.title, icon: info.icon, script: info.script, install: info.install }, projects };
  });
}

async function newestInstall(root, install, script) {
  let names;
  try {
    names = await readdir(root);
  } catch {
    return null;
  }
  // `PyCharm 2025.2.4`, or `PyCharm` from Toolbox; never `PyCharm Community Edition` for `PyCharm`.
  const pattern = new RegExp(`^${install.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}( \\d[\\d.]*)?$`);
  const candidates = names.filter((name) => pattern.test(name)).sort().reverse();
  for (const name of candidates) {
    const program = join(root, name, "bin", `${script}64.exe`);
    try {
      if (await exists(program)) return program;
    } catch {
      // Keep looking.
    }
  }
  return null;
}

/** Where this IDE's program is on Windows: a Toolbox script, a Toolbox install, or a standalone one. */
export async function findJetBrainsExecutable(editor) {
  const { home_path } = environment();
  const local = join(home_path, "AppData", "Local");
  const script = join(local, "JetBrains", "Toolbox", "scripts", `${editor.script}.cmd`);
  try {
    if (await exists(script)) return script;
  } catch {
    // Not granted, or no Toolbox.
  }
  return (
    (await newestInstall(join(local, "Programs"), editor.install, editor.script)) ??
    (await newestInstall("C:\\Program Files\\JetBrains", editor.install, editor.script))
  );
}
