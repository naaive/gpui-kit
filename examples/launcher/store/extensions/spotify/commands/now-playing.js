// Shows the track Spotify is playing, with its artwork, and controls it:
// play or pause, next and previous, Liked Songs, shuffle and repeat. With no
// active device it says so and offers to open Spotify.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  Detail,
  MetadataLabel,
  MetadataLink,
  MetadataSeparator,
  MetadataTags,
} from "launcher";
import { show_toast } from "launcher/api";
import {
  cycleRepeat,
  isLiked,
  like,
  next,
  nowPlaying,
  pause,
  play,
  playback,
  previous,
  setShuffle,
  unlike,
} from "../lib/player.js";
import { signInPage } from "../lib/sign-in.js";
import { describe, formatDuration, isSignedIn, webUrl } from "../lib/spotify.js";

const REPEAT_TITLES = { off: "Off", context: "All", track: "One Track" };

export default class NowPlaying extends View {
  init(props, cx) {
    this.signed_in = isSignedIn();
    this.track = null;
    this.liked = false;
    this.loading = false;
    this.error = null;
    this.no_device = false;
    if (this.signed_in) this.refresh(cx);
  }

  refresh(cx, delay_ms = 0) {
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        if (delay_ms > 0) await task.sleep(delay_ms);
        const track = nowPlaying(await playback());
        this.track = track;
        this.no_device = track === null;
        this.error = null;
        this.liked = track && !track.episode ? await isLiked(track) : false;
      } catch (error) {
        this.track = null;
        this.no_device = Boolean(error?.no_device);
        this.error = describe(error);
        if (error?.signed_out) this.signed_in = false;
      } finally {
        this.loading = false;
        task.notify();
      }
    });
  }

  /** Runs a player command, then reads the state again once Spotify has applied it. */
  control(cx, work, done = null) {
    cx.spawn(async (task) => {
      try {
        const result = await work();
        if (done) show_toast({ title: done(result), style: "success" });
        this.refresh(task, 500);
      } catch (error) {
        this.no_device = Boolean(error?.no_device);
        show_toast({ title: "Spotify did not respond", message: describe(error), style: "failure" });
      }
    });
  }

  markdown() {
    if (this.error || !this.track) {
      const lines = this.no_device
        ? [
            "# Nothing Is Playing",
            "",
            "Spotify has no active device. Open Spotify on this computer, your phone or a speaker,",
            "play something, then choose **Refresh**.",
          ]
        : this.loading
          ? ["# Now Playing", "", "Asking Spotify…"]
          : ["# Cannot Reach Spotify", "", this.error ?? "Spotify answered nothing."];
      return lines.join("\n");
    }
    const { name, artists, album, image, progress_ms, duration_ms, is_playing } = this.track;
    return [
      image ? `![${album}](${image})\n` : "",
      `# ${name}`,
      "",
      `**${artists}**${album ? ` · ${album}` : ""}`,
      "",
      `${is_playing ? "Playing" : "Paused"} · ${formatDuration(progress_ms)} / ${formatDuration(duration_ms)}`,
    ].join("\n");
  }

  metadata() {
    const track = this.track;
    if (!track) return [];
    const modes = new MetadataTags("Modes")
      .tag(track.shuffle ? "Shuffle On" : "Shuffle Off", track.shuffle ? "accent" : "neutral")
      .tag(`Repeat ${REPEAT_TITLES[track.repeat] ?? track.repeat}`, track.repeat === "off" ? "neutral" : "accent");
    return [
      new MetadataLabel("Artist", track.artists || "Unknown"),
      ...(track.album ? [new MetadataLabel(track.episode ? "Show" : "Album", track.album)] : []),
      new MetadataLabel("Length", formatDuration(track.duration_ms)),
      new MetadataLabel("Device", track.device || "Unknown"),
      ...(track.volume !== null ? [new MetadataLabel("Volume", `${track.volume}%`)] : []),
      modes,
      ...(track.episode ? [] : [new MetadataLabel("Liked", this.liked ? "Yes" : "No")]),
      new MetadataSeparator(),
      new MetadataLink("Link", "open.spotify.com", track.url || webUrl(track.uri)),
    ];
  }

  actions() {
    const refresh = new Action("Refresh").icon("refresh-cw").shortcut("secondary-r").run((cx) => this.refresh(cx));
    const openSpotify = new Action("Open Spotify").icon("external-link").open_url("spotify:");
    const track = this.track;
    if (!track) return new ActionPanel().children(this.no_device ? [openSpotify, refresh] : [refresh, openSpotify]);
    const link = track.url || webUrl(track.uri);
    return new ActionPanel().children([
      track.is_playing
        ? new Action("Pause").icon("pause").run((cx) => this.control(cx, () => pause()))
        : new Action("Play").icon("play").run((cx) => this.control(cx, () => play())),
      new Action("Next Track").icon("skip-forward").shortcut("secondary-shift-n").run((cx) => this.control(cx, () => next())),
      new Action("Previous Track").icon("skip-back").shortcut("secondary-shift-b").run((cx) => this.control(cx, () => previous())),
      new ActionPanelSection("Track").children([
        ...(track.episode
          ? []
          : [
              this.liked
                ? new Action("Remove from Liked Songs")
                    .icon("heart-off")
                    .shortcut("secondary-shift-l")
                    .run((cx) => this.control(cx, () => unlike(track), () => "Removed from Liked Songs"))
                : new Action("Like").icon("heart").shortcut("secondary-shift-l").run((cx) =>
                    this.control(cx, () => like(track), () => "Added to Liked Songs"),
                  ),
            ]),
        new Action("Open in Spotify").icon("external-link").shortcut("secondary-shift-o").open_url(track.uri),
        new Action("Copy Link").icon("link").shortcut("secondary-shift-c").copy(link),
      ]),
      new ActionPanelSection("Playback").children([
        new Action(track.shuffle ? "Turn Shuffle Off" : "Turn Shuffle On")
          .icon("shuffle")
          .shortcut("secondary-shift-s")
          .run((cx) => this.control(cx, () => setShuffle(!track.shuffle), () => (track.shuffle ? "Shuffle off" : "Shuffle on"))),
        new Action(`Repeat: ${REPEAT_TITLES[track.repeat] ?? "Off"}`)
          .icon(track.repeat === "track" ? "repeat-1" : "repeat")
          .shortcut("secondary-shift-t")
          .run((cx) => this.control(cx, () => cycleRepeat(track.repeat), (mode) => `Repeat ${REPEAT_TITLES[mode].toLowerCase()}`)),
        refresh,
      ]),
    ]);
  }

  render() {
    if (!this.signed_in) {
      return signInPage((cx) => {
        this.signed_in = true;
        this.refresh(cx);
      });
    }
    return new Detail(this.markdown()).loading(this.loading).children(this.metadata()).actions(this.actions());
  }
}
