// A no-view command: it copies today's date and reports with a HUD, which the
// launcher shows after its window hides. Copying needs no capability, because
// the launcher performs it.
import { View } from "gpui-kit";
import { List } from "launcher";
import { copy, show_hud } from "launcher/api";

function isoDate(date) {
  const pad = (value) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

export default class CopyDate extends View {
  init() {
    const today = isoDate(new Date());
    copy(today);
    show_hud(`Copied ${today}`);
  }

  render() {
    return new List();
  }
}
