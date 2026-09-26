// A no-view command with an argument, also offered as a fallback: when the
// root search finds nothing, "Search GPUI Kit Docs" appears with the query as
// its `query` argument. The work happens in `init`; `render` is never shown.
import { View } from "gpui-kit";
import { List } from "launcher";
import { close_main_window, launch, open } from "launcher/api";

export default class SearchDocs extends View {
  init() {
    const query = (launch().arguments.query ?? "").trim();
    const terms = encodeURIComponent(`site:gpui-kit.com ${query}`.trim());
    open(query ? `https://duckduckgo.com/?q=${terms}` : "https://gpui-kit.com/docs");
    close_main_window();
  }

  render() {
    return new List();
  }
}
