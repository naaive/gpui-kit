// Goes back to the previous track and shows it in a HUD once Spotify has moved.
import { View } from "gpui-kit";
import { List } from "launcher";
import { nowPlaying, playback, previous, settledTrack } from "../lib/player.js";
import { runPlaybackCommand, trackLine } from "../lib/no-view.js";

export default class PreviousTrack extends View {
  init(props, cx) {
    runPlaybackCommand(cx, async (task) => {
      const before = nowPlaying(await playback());
      await previous();
      // Spotify restarts the track instead when it has played a few seconds.
      const after = await settledTrack(task, before?.id ?? null);
      return `⏮ ${trackLine(after ?? before, "Previous track")}`;
    });
  }

  render() {
    return new List();
  }
}
