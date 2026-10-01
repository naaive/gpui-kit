// Searches crates.io as the user types, with a detail pane per crate and
// actions to copy `cargo add` or open its documentation. crates.io refuses
// requests without a User-Agent that names the client, so one is sent.
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  Detail,
  List,
  ListItem,
  MetadataLabel,
  MetadataLink,
  MetadataSeparator,
} from "launcher";
import { SearchView, formatCount, formatDate, getJson, plain } from "../lib/search-view.js";

const SEARCH_URL = "https://crates.io/api/v1/crates";
const USER_AGENT = "gpui-kit-launcher-package-search (https://github.com/longbridge/gpui-component)";

function toCrate(crate) {
  return {
    id: crate.id ?? crate.name,
    name: crate.name,
    description: (crate.description ?? "").trim(),
    version: crate.max_stable_version ?? crate.max_version ?? crate.newest_version ?? "",
    downloads: crate.downloads ?? null,
    recent: crate.recent_downloads ?? null,
    updated: crate.updated_at ?? null,
    created: crate.created_at ?? null,
    homepage: crate.homepage ?? null,
    repository: crate.repository ?? null,
    documentation: crate.documentation ?? `https://docs.rs/${crate.name}`,
    page: `https://crates.io/crates/${crate.name}`,
  };
}

function detail(crate) {
  const metadata = [
    new MetadataLabel("Version", crate.version || "Unknown"),
    new MetadataLabel("Downloads", formatCount(crate.downloads)),
    new MetadataLabel("Recent Downloads", formatCount(crate.recent)),
    new MetadataLabel("Updated", formatDate(crate.updated)),
    new MetadataLabel("Created", formatDate(crate.created)),
    new MetadataSeparator(),
    new MetadataLink("crates.io", crate.name, crate.page),
    new MetadataLink("Documentation", crate.documentation.replace(/^https?:\/\//, ""), crate.documentation),
  ];
  if (crate.repository) metadata.push(new MetadataLink("Repository", crate.repository.replace(/^https?:\/\//, ""), crate.repository));
  if (crate.homepage) metadata.push(new MetadataLink("Homepage", crate.homepage.replace(/^https?:\/\//, ""), crate.homepage));
  const add = `\`\`\`sh\ncargo add ${crate.name}\n\`\`\``;
  return new Detail(`# ${plain(crate.name)}\n\n${plain(crate.description)}\n\n${add}`).children(metadata);
}

export default class SearchCrates extends SearchView {
  get registry() {
    return "crates.io";
  }

  get placeholder() {
    return "Search crates…";
  }

  nothing(query) {
    return ["No crates found", `Nothing on crates.io matches “${query}”.`];
  }

  async search(query) {
    const url = `${SEARCH_URL}?q=${encodeURIComponent(query)}&per_page=20`;
    const { crates } = await getJson(url, "crates.io", { "User-Agent": USER_AGENT });
    return (crates ?? []).map(toCrate);
  }

  row(crate) {
    const more = [new Action("Copy Crate Name").icon("copy").shortcut("secondary-shift-n").copy(crate.name)];
    if (crate.repository) more.unshift(new Action("Open Repository").icon("github").shortcut("secondary-shift-o").open_url(crate.repository));
    if (crate.homepage) more.unshift(new Action("Open Homepage").icon("globe").open_url(crate.homepage));
    const item = new ListItem(crate.id, crate.name).icon("box").subtitle(crate.description);
    return (crate.version ? item.accessory(`v${crate.version}`) : item)
      .detail(detail(crate))
      .actions(
        new ActionPanel().children([
          new Action("Copy cargo add Command").icon("copy").copy(`cargo add ${crate.name}`),
          new Action("Open docs.rs").icon("book-open").shortcut("secondary-shift-d").open_url(`https://docs.rs/${crate.name}`),
          new Action("Open on crates.io").icon("external-link").open_url(crate.page),
          new ActionPanelSection("More").children([
            new Action("Copy Cargo.toml Line").icon("copy").copy(`${crate.name} = "${crate.version}"`),
            ...more,
          ]),
        ]),
      );
  }

  render() {
    return this.page(new List());
  }
}
