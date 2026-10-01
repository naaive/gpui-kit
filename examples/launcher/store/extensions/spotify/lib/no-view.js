// What the no-view playback commands share: before signing in they open Now
// Playing, which shows the setup page; otherwise they run one player command
// and report with a HUD, which also tells the launcher they are done.
import { launch_command, show_hud } from "launcher/api";
import { describe, isSignedIn } from "./spotify.js";

/** `work(cx)` performs the command and answers the HUD text. */
export function runPlaybackCommand(cx, work) {
  if (!isSignedIn()) {
    launch_command("now-playing");
    return;
  }
  cx.spawn(async (task) => {
    try {
      show_hud(await work(task));
    } catch (error) {
      show_hud(describe(error));
    }
  });
}

/** “Song — Artist”, or a fallback when Spotify has not said yet. */
export function trackLine(track, fallback) {
  return track ? `${track.name} — ${track.artists}` : fallback;
}
