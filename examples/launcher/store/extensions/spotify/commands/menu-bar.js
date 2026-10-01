// Spotify in the system tray (the menu bar on macOS): the playing track as
// the title or tooltip, and playback controls as menu items. The launcher
// runs it again every 30 seconds; a control reads the state again right away.
import { View } from "gpui-kit";
import { Action, MenuBarExtra, MenuBarItem, MenuBarSection, MenuBarSeparator } from "launcher";
import { cycleRepeat, like, next, nowPlaying, pause, play, playback, previous, setShuffle } from "../lib/player.js";
import { describe, isSignedIn, webUrl } from "../lib/spotify.js";

const REPEAT_TITLES = { off: "Off", context: "All", track: "One Track" };
const TITLE_LIMIT = 32;

function shorten(text) {
  return text.length > TITLE_LIMIT ? `${text.slice(0, TITLE_LIMIT - 1)}…` : text;
}

export default class NowPlayingInTray extends View {
  init(props, cx) {
    this.signed_in = isSignedIn();
    this.track = null;
    this.error = null;
    this.no_device = false;
    this.loading = this.signed_in;
    if (this.signed_in) this.refresh(cx);
  }

  refresh(cx, delay_ms = 0) {
    cx.spawn(async (task) => {
      try {
        if (delay_ms > 0) await task.sleep(delay_ms);
        this.track = nowPlaying(await playback());
        this.no_device = this.track === null;
        this.error = null;
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

  control(title, work) {
    return new MenuBarItem(title).action(
      new Action(title).run((cx) => {
        cx.spawn(async (task) => {
          try {
            await work();
          } catch (error) {
            this.error = describe(error);
          }
          this.refresh(task, 500);
        });
      }),
    );
  }

  signedOutMenu() {
    return new MenuBarExtra()
      .icon("music")
      .tooltip("Spotify: not signed in")
      .children([
        new MenuBarItem("Sign In to Spotify…").action(new Action("Sign In").launch("now-playing")),
        new MenuBarItem("Open Spotify").action(new Action("Open Spotify").open_url("spotify:")),
      ]);
  }

  render() {
    if (!this.signed_in) return this.signedOutMenu();
    const track = this.track;
    const line = track ? `${track.name} — ${track.artists}` : "";
    const status = track
      ? `${track.is_playing ? "Playing" : "Paused"} on ${track.device || "Spotify"}`
      : this.no_device
        ? "No active device"
        : this.error ?? "Nothing playing";
    const menu = new MenuBarExtra()
      .icon(track?.is_playing ? "audio-lines" : "music")
      .title(track?.is_playing ? shorten(track.name) : "")
      .tooltip(track ? `${line} (${status})` : `Spotify: ${status}`)
      .loading(this.loading);
    if (!track) {
      return menu.children([
        this.no_device ? new MenuBarItem(status).subtitle("Play something in Spotify first") : new MenuBarItem(status),
        new MenuBarSeparator(),
        new MenuBarItem("Open Spotify").action(new Action("Open Spotify").open_url("spotify:")),
        this.control("Refresh", async () => {}),
      ]);
    }
    return menu.children([
      new MenuBarSection(status).children([
        new MenuBarItem(track.name)
          .subtitle(track.artists)
          .action(new Action("Show Now Playing").launch("now-playing")),
      ]),
      new MenuBarSection("Playback").children([
        track.is_playing ? this.control("Pause", () => pause()) : this.control("Play", () => play()),
        this.control("Next Track", () => next()),
        this.control("Previous Track", () => previous()),
      ]),
      new MenuBarSection("Options").children([
        this.control("Shuffle", () => setShuffle(!track.shuffle)).checked(track.shuffle),
        this.control(`Repeat: ${REPEAT_TITLES[track.repeat] ?? "Off"}`, () => cycleRepeat(track.repeat)),
        ...(track.episode ? [] : [this.control("Like", () => like(track))]),
      ]),
      new MenuBarSeparator(),
      new MenuBarItem("Open in Spotify").action(new Action("Open in Spotify").open_url(track.uri)),
      new MenuBarItem("Copy Link").action(new Action("Copy Link").copy(track.url || webUrl(track.uri))),
    ]);
  }
}
