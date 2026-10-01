// A no-view command with an `interval`: once the user has opened it, the
// launcher runs it every hour on its own (`launch().launch_type` is then
// "background"). It shows its answer as the command's subtitle in the root
// search, and only reports with a HUD when the user opened it.
import { View } from "gpui-kit";
import { List } from "launcher";
import { launch, show_hud, update_command_metadata } from "launcher/api";

export default class Weekend extends View {
  init() {
    const today = new Date().getDay();
    const days = today === 6 || today === 0 ? 0 : 6 - today;
    const text = days === 0 ? "It's the weekend" : `${days} day${days === 1 ? "" : "s"} to go`;
    update_command_metadata({ subtitle: text });
    if (launch().launch_type === "user_initiated") {
      show_hud(text);
    }
  }

  render() {
    return new List();
  }
}
