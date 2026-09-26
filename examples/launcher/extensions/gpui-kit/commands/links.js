// A static page: sections, items, and actions the launcher performs itself.
// Opening a link or copying text needs no capability, because the extension
// only describes the action; it never runs code to do it.
import { View } from "gpui-kit";
import { Action, List, ListItem, ListSection } from "launcher";

const DOCUMENTATION = [
  { id: "home", title: "GPUI Kit", url: "https://gpui-kit.com", icon: "globe" },
  { id: "design", title: "Design Guides", url: "https://gpui-kit.com/docs/design-guides", icon: "palette" },
  { id: "coding", title: "Coding Guides", url: "https://gpui-kit.com/docs/coding-guides", icon: "file-text" },
];

const COMMUNITY = [
  { id: "github", title: "GitHub Repository", url: "https://github.com/longbridge/gpui-component", icon: "github" },
  { id: "issues", title: "Report an Issue", url: "https://github.com/longbridge/gpui-component/issues", icon: "circle-alert" },
];

function link({ id, title, url, icon }) {
  return new ListItem(id, title)
    .subtitle(url.replace("https://", ""))
    .icon(icon)
    .action(new Action("Open in Browser").open_url(url))
    .action(new Action("Copy URL").shortcut("secondary-shift-c").copy(url));
}

export default class Links extends View {
  render() {
    return new List()
      .placeholder("Search GPUI Kit links…")
      .child(new ListSection("Documentation").children(DOCUMENTATION.map(link)))
      .child(new ListSection("Community").children(COMMUNITY.map(link)));
  }
}
