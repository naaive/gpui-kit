// A static page: sections, items, and actions the launcher performs itself.
// Opening a link or copying text needs no capability, because the extension
// only describes the action; it never runs code to do it.
//
// "Show Details" pushes a page of its own: `push` takes a function returning
// a View, and that View's `render` becomes the new page.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  Detail,
  List,
  ListItem,
  ListSection,
  MetadataLabel,
  MetadataLink,
} from "launcher";

const DOCUMENTATION = [
  { id: "home", title: "GPUI Kit", url: "https://gpui-kit.com", icon: "globe" },
  { id: "design", title: "Design Guides", url: "https://gpui-kit.com/docs/design-guides", icon: "palette" },
  { id: "coding", title: "Coding Guides", url: "https://gpui-kit.com/docs/coding-guides", icon: "file-text" },
];

const COMMUNITY = [
  { id: "github", title: "GitHub Repository", url: "https://github.com/longbridge/gpui-component", icon: "github" },
  { id: "issues", title: "Report an Issue", url: "https://github.com/longbridge/gpui-component/issues", icon: "circle-alert" },
];

class LinkDetail extends View {
  init(link) {
    this.link = link;
  }

  render() {
    const { title, url } = this.link;
    return new Detail(`# ${title}\n\n${url}`)
      .actions(new ActionPanel().child(new Action("Open in Browser").open_url(url)))
      .children([new MetadataLabel("Title", title), new MetadataLink("Address", url.replace("https://", ""), url)]);
  }
}

function link(entry) {
  const { id, title, url, icon } = entry;
  return new ListItem(id, title)
    .subtitle(url.replace("https://", ""))
    .icon(icon)
    .actions(
      new ActionPanel().children([
        new Action("Open in Browser").open_url(url),
        new Action("Copy URL").shortcut("secondary-shift-c").copy(url),
        new ActionPanelSection("More").children([
          new Action("Copy as Markdown").copy(`[${title}](${url})`),
          new Action("Show Details").icon("info").shortcut("secondary-i").push(() => new LinkDetail(entry), title),
        ]),
      ]),
    );
}

export default class Links extends View {
  render() {
    return new List()
      .placeholder("Search GPUI Kit links…")
      .child(new ListSection("Documentation").children(DOCUMENTATION.map(link)))
      .child(new ListSection("Community").children(COMMUNITY.map(link)));
  }
}
