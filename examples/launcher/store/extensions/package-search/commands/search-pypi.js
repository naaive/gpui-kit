// Looks up a PyPI package by its exact name as the user types. PyPI has no
// JSON search API (its only search is the HTML website, and the full simple
// index is tens of megabytes), so this asks `/pypi/<name>/json` for the name
// typed, normalized the way PyPI normalizes names.
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  ActionPanelSubmenu,
  Detail,
  List,
  ListItem,
  MetadataLabel,
  MetadataLink,
  MetadataSeparator,
} from "launcher";
import { SearchView, browsableRepository, plain } from "../lib/search-view.js";

const PROJECT_URL = "https://pypi.org/pypi";

/** PEP 503: lowercase, with runs of `-`, `_` and `.` as one `-`. */
function normalize(name) {
  return name.trim().toLowerCase().replace(/[-_.]+/g, "-");
}

function isName(name) {
  return /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(name);
}

function author(info) {
  if (info.author) return info.author;
  const email = info.author_email ?? info.maintainer_email ?? "";
  // "Ada Lovelace <ada@example.com>, …" → "Ada Lovelace"
  const named = email.match(/^\s*([^<,]+?)\s*</);
  return named ? named[1] : null;
}

function links(info) {
  const found = [];
  const seen = new Set();
  const add = (label, url) => {
    const browsable = browsableRepository(url);
    if (!browsable || seen.has(browsable)) return;
    seen.add(browsable);
    found.push({ label, url: browsable });
  };
  add("Homepage", info.home_page);
  for (const [label, url] of Object.entries(info.project_urls ?? {})) add(label, url);
  return found.slice(0, 6);
}

function toPackage(info) {
  const project = links(info);
  const source = project.find(({ label, url }) => /source|repo|code|github/i.test(label) || /github\.com|gitlab\.com/.test(url));
  return {
    id: normalize(info.name),
    name: info.name,
    summary: (info.summary ?? "").trim(),
    version: info.version ?? "",
    license: info.license_expression || (info.license && info.license.length < 40 ? info.license : null),
    python: info.requires_python || null,
    author: author(info),
    page: info.package_url ?? `https://pypi.org/project/${info.name}/`,
    links: project,
    source: source?.url ?? null,
  };
}

function detail(pkg) {
  const metadata = [
    new MetadataLabel("Version", pkg.version || "Unknown"),
    new MetadataLabel("Requires Python", pkg.python ?? "Any"),
    new MetadataLabel("License", pkg.license ?? "Unknown"),
  ];
  if (pkg.author) metadata.push(new MetadataLabel("Author", pkg.author));
  metadata.push(new MetadataSeparator(), new MetadataLink("PyPI", pkg.name, pkg.page));
  for (const { label, url } of pkg.links) metadata.push(new MetadataLink(label, url.replace(/^https?:\/\//, ""), url));
  const install = `\`\`\`sh\npip install ${pkg.name}\n\`\`\``;
  return new Detail(`# ${plain(pkg.name)}\n\n${plain(pkg.summary)}\n\n${install}`).children(metadata);
}

export default class SearchPyPI extends SearchView {
  get registry() {
    return "PyPI";
  }

  get placeholder() {
    return "Exact package name…";
  }

  get prompt() {
    return ["Look Up a PyPI Package", "Type a package's exact name, such as requests or numpy."];
  }

  nothing(query) {
    return [`No package named “${query}”`, "PyPI has no search API, so the name must match exactly."];
  }

  async search(query) {
    const name = normalize(query);
    if (!isName(name)) return [];
    const response = await fetch(`${PROJECT_URL}/${name}/json`, { headers: { Accept: "application/json" } });
    if (response.status === 404) return [];
    if (!response.ok) throw new Error(`PyPI answered ${response.status}.`);
    const { info } = await response.json();
    return info ? [toPackage(info)] : [];
  }

  row(pkg) {
    const more = pkg.links.map(({ label, url }) => new Action(`Open ${label}`).icon("globe").open_url(url));
    more.push(new Action("Copy Package Name").icon("copy").shortcut("secondary-shift-n").copy(pkg.name));
    const item = new ListItem(pkg.id, pkg.name).icon("package-2").subtitle(pkg.summary);
    return (pkg.version ? item.accessory(`v${pkg.version}`) : item)
      .detail(detail(pkg))
      .actions(
        new ActionPanel().children([
          new Action("Copy pip Install Command").icon("copy").copy(`pip install ${pkg.name}`),
          new Action("Open on PyPI").icon("external-link").shortcut("secondary-shift-p").open_url(pkg.page),
          ...(pkg.source
            ? [new Action("Open Repository").icon("github").shortcut("secondary-shift-o").open_url(pkg.source)]
            : []),
          new ActionPanelSubmenu("Copy Install Command For…").icon("terminal").children([
            new Action("uv").copy(`uv add ${pkg.name}`),
            new Action("Poetry").copy(`poetry add ${pkg.name}`),
            new Action("pipx").copy(`pipx install ${pkg.name}`),
            new Action("pip (Pinned Version)").copy(`pip install ${pkg.name}==${pkg.version}`),
          ]),
          new ActionPanelSection("More").children(more),
        ]),
      );
  }

  render() {
    return this.page(new List());
  }
}
