// Pauses Spotify if it plays, or resumes it if it is paused, and shows the
// track in a HUD. Opens Now Playing to sign in first if needed.
import { View } from "gpui-kit";
import { List } from "launcher";
import { togglePlayback } from "../lib/player.js";
import { runPlaybackCommand, trackLine } from "../lib/no-view.js";

export default class PlayPause extends View {
  init(props, cx) {
    runPlaybackCommand(cx, async () => {
      const { playing, track } = await togglePlayback();
      return `${playing ? "▶" : "⏸"} ${trackLine(track, playing ? "Playing" : "Paused")}`;
    });
  }

  render() {
    return new List();
  }
}
