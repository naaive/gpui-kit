// Skips to the next track and shows it in a HUD once Spotify has moved on.
import { View } from "gpui-kit";
import { List } from "launcher";
import { next, nowPlaying, playback, settledTrack } from "../lib/player.js";
import { runPlaybackCommand, trackLine } from "../lib/no-view.js";

export default class NextTrack extends View {
  init(props, cx) {
    runPlaybackCommand(cx, async (task) => {
      const before = nowPlaying(await playback());
      await next();
      return `⏭ ${trackLine(await settledTrack(task, before?.id ?? null), "Next track")}`;
    });
  }

  render() {
    return new List();
  }
}
