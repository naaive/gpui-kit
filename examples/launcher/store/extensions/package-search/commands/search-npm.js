// Searches the npm registry as the user types, with a detail pane per
// package and actions to open it or copy the command that installs it.
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
  MetadataTags,
} from "launcher";
import { SearchView, browsableRepository, formatCount, formatDate, getJson, plain } from "../lib/search-view.js";

const SEARCH_URL = "https://registry.npmjs.org/-/v1/search";

function toPackage({ package: pkg, downloads }) {
  const links = pkg.links ?? {};
  return {
    id: pkg.name,
    name: pkg.name,
    description: pkg.description ?? "",
    version: pkg.version ?? "",
    license: pkg.license ?? null,
    publisher: pkg.publisher?.username ?? null,
    date: pkg.date ?? null,
    keywords: Array.isArray(pkg.keywords) ? pkg.keywords.slice(0, 8) : [],
    weekly: downloads?.weekly ?? null,
    npm: links.npm ?? `https://www.npmjs.com/package/${pkg.name}`,
    homepage: links.homepage ?? null,
    repository: browsableRepository(links.repository),
  };
}

function detail(pkg) {
  const metadata = [
    new MetadataLabel("Version", pkg.version || "Unknown"),
    new MetadataLabel("Weekly Downloads", pkg.weekly == null ? "Unknown" : formatCount(pkg.weekly)),
    new MetadataLabel("License", pkg.license ?? "None"),
    new MetadataLabel("Published", formatDate(pkg.date)),
  ];
  if (pkg.publisher) metadata.push(new MetadataLabel("Publisher", pkg.publisher));
  if (pkg.keywords.length > 0) {
    metadata.push(pkg.keywords.reduce((tags, keyword) => tags.tag(keyword), new MetadataTags("Keywords")));
  }
  metadata.push(new MetadataSeparator(), new MetadataLink("npm", pkg.name, pkg.npm));
  if (pkg.repository) metadata.push(new MetadataLink("Repository", pkg.repository.replace(/^https?:\/\//, ""), pkg.repository));
  if (pkg.homepage) metadata.push(new MetadataLink("Homepage", pkg.homepage.replace(/^https?:\/\//, ""), pkg.homepage));
  const install = `\`\`\`sh\nnpm install ${pkg.name}\n\`\`\``;
  return new Detail(`# ${plain(pkg.name)}\n\n${plain(pkg.description)}\n\n${install}`).children(metadata);
}

export default class SearchNpm extends SearchView {
  get registry() {
    return "npm";
  }

  get placeholder() {
    return "Search npm packages…";
  }

  async search(query) {
    const { objects } = await getJson(`${SEARCH_URL}?text=${encodeURIComponent(query)}&size=20`, "npm");
    return (objects ?? []).map(toPackage);
  }

  row(pkg) {
    const more = [];
    if (pkg.homepage) more.push(new Action("Open Homepage").icon("globe").open_url(pkg.homepage));
    more.push(new Action("Copy Package Name").icon("copy").shortcut("secondary-shift-n").copy(pkg.name));
    const item = new ListItem(pkg.id, pkg.name).icon("package").subtitle(pkg.description);
    return (pkg.version ? item.accessory(`v${pkg.version}`) : item)
      .detail(detail(pkg))
      .actions(
        new ActionPanel().children([
          new Action("Open on npmjs.com").icon("external-link").open_url(pkg.npm),
          ...(pkg.repository
            ? [new Action("Open Repository").icon("github").shortcut("secondary-shift-o").open_url(pkg.repository)]
            : []),
          new Action("Copy npm Install Command").icon("copy").shortcut("secondary-shift-c").copy(`npm install ${pkg.name}`),
          new ActionPanelSubmenu("Copy Install Command For…").icon("terminal").children([
            new Action("pnpm").copy(`pnpm add ${pkg.name}`),
            new Action("Yarn").copy(`yarn add ${pkg.name}`),
            new Action("Bun").copy(`bun add ${pkg.name}`),
            new Action("npm (Dev Dependency)").copy(`npm install --save-dev ${pkg.name}`),
          ]),
          new ActionPanelSection("More").children(more),
        ]),
      );
  }

  render() {
    return this.page(new List());
  }
}
