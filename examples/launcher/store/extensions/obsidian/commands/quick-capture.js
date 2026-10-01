// A no-view command: appends what was typed as its argument, with a
// timestamp, to the inbox note (or today's daily note) of the default vault,
// and reports with a HUD. The note and its folders are created if missing.
import { View } from "gpui-kit";
import { List } from "launcher";
import { launch, show_hud } from "launcher/api";
import { appendToNote, dailyNotePath, formatMoment, loadVaults, pickVault, timestamp } from "../lib/vaults.js";

function inboxPath(preference) {
  const path = String(preference ?? "").trim().replace(/^[\\/]+/, "") || "Inbox.md";
  return path.toLowerCase().endsWith(".md") ? path : `${path}.md`;
}

export default class QuickCapture extends View {
  init(_props, cx) {
    const { arguments: args, preferences } = launch();
    const text = String(args.text ?? "").trim();
    if (!text) {
      show_hud("Type something to capture");
      return;
    }
    cx.spawn(async () => {
      try {
        const vault = pickVault(await loadVaults(), preferences.defaultVault);
        if (!vault) {
          show_hud("No Obsidian vault found");
          return;
        }
        const now = new Date();
        const daily = preferences.captureTarget === "daily";
        const relative = daily ? await dailyNotePath(vault, now) : inboxPath(preferences.inboxNote);
        const line = daily ? `- ${formatMoment(now, "HH:mm")} ${text}` : `- ${timestamp(now)} ${text}`;
        await appendToNote(vault, relative, line);
        show_hud(`Captured to ${relative.replace(/\.md$/i, "")}`);
      } catch (error) {
        show_hud(`Cannot capture: ${String(error?.message ?? error)}`);
      }
    });
  }

  render() {
    return new List();
  }
}
