// Searches Spotify as the user types, for tracks, albums, artists or
// playlists (the dropdown picks which), and plays, queues or opens a result.
// Before signing in it shows the setup page instead.
import { View } from "gpui-kit";
import {
  Action,
  ActionPanel,
  ActionPanelSection,
  List,
  ListDropdown,
  ListDropdownItem,
  ListItem,
} from "launcher";
import { launch, show_toast } from "launcher/api";
import { addToQueue, play } from "../lib/player.js";
import { signInPage } from "../lib/sign-in.js";
import { api, artistNames, artwork, describe, formatDuration, isSignedIn, webUrl } from "../lib/spotify.js";

const TYPING_PAUSE_MS = 300;
const TYPES = [
  { value: "track", title: "Tracks", icon: "music" },
  { value: "album", title: "Albums", icon: "disc-3" },
  { value: "artist", title: "Artists", icon: "mic-vocal" },
  { value: "playlist", title: "Playlists", icon: "list-music" },
];

function describeResult(type, item) {
  switch (type) {
    case "track":
      return {
        subtitle: artistNames(item.artists),
        accessory: formatDuration(item.duration_ms),
        image: artwork(item.album?.images, 300),
      };
    case "album":
      return {
        subtitle: artistNames(item.artists),
        accessory: String(item.release_date ?? "").slice(0, 4),
        image: artwork(item.images, 300),
      };
    case "artist":
      return {
        subtitle: (item.genres ?? []).slice(0, 2).join(", "),
        accessory: "",
        image: artwork(item.images, 300),
      };
    default:
      return {
        subtitle: item.owner?.display_name ?? "",
        accessory: item.tracks?.total ? `${item.tracks.total} tracks` : "",
        image: artwork(item.images, 300),
      };
  }
}

async function search(query, type) {
  const answer = await api("GET", "/search", { query: { q: query, type, limit: "10" } });
  // Spotify sometimes answers `null` in place of a playlist it will not show.
  return (answer?.[`${type}s`]?.items ?? []).filter((item) => item && item.uri);
}

export default class SearchSpotify extends View {
  init(props, cx) {
    this.type = "track";
    this.query = "";
    this.results = [];
    this.loading = false;
    this.error = null;
    this.generation = 0;
    this.pending = null;
    this.signed_in = isSignedIn();
    const query = String(launch().arguments.query ?? "").trim();
    if (query && this.signed_in) {
      this.query = query;
      this.search(cx);
    }
  }

  typed(query, cx) {
    this.query = query.trim();
    this.pending?.cancel();
    this.generation += 1;
    this.error = null;
    this.loading = this.query !== "";
    if (!this.query) this.results = [];
    this.pending = this.query ? cx.timer.after(TYPING_PAUSE_MS, (timer) => this.search(timer)) : null;
    cx.notify();
  }

  search(cx) {
    const run = ++this.generation;
    const { query, type } = this;
    this.loading = true;
    cx.spawn(async (task) => {
      try {
        const results = await search(query, type);
        if (run !== this.generation) return;
        this.results = results;
        this.error = null;
      } catch (error) {
        if (run !== this.generation) return;
        this.results = [];
        this.error = describe(error);
        if (error?.signed_out) this.signed_in = false;
      } finally {
        if (run === this.generation) {
          this.loading = false;
          task.notify();
        }
      }
    });
  }

  perform(cx, title, work) {
    cx.spawn(async () => {
      try {
        await work();
        show_toast({ title, style: "success" });
      } catch (error) {
        show_toast({ title: "Spotify did not respond", message: describe(error), style: "failure" });
      }
    });
  }

  actions(item) {
    const url = item.external_urls?.spotify ?? webUrl(item.uri);
    const playTarget = this.type === "track" ? { uris: [item.uri] } : { context_uri: item.uri };
    return new ActionPanel().children([
      new Action("Play").icon("play").run((cx) => this.perform(cx, `Playing ${item.name}`, () => play(playTarget))),
      ...(this.type === "track"
        ? [
            new Action("Add to Queue")
              .icon("list-music")
              .shortcut("secondary-shift-q")
              .run((cx) => this.perform(cx, `Queued ${item.name}`, () => addToQueue(item.uri))),
          ]
        : []),
      new Action("Open in Spotify").icon("external-link").shortcut("secondary-shift-o").open_url(item.uri),
      new Action("Open in Browser").icon("globe").open_url(url),
      new ActionPanelSection("Copy").children([
        new Action("Copy Link").icon("link").shortcut("secondary-shift-c").copy(url),
        new Action("Copy Spotify URI").icon("copy").copy(item.uri),
      ]),
    ]);
  }

  row(item) {
    const { subtitle, accessory, image } = describeResult(this.type, item);
    const fallback = TYPES.find((type) => type.value === this.type).icon;
    let row = new ListItem(item.uri, item.name).icon(image ?? fallback);
    if (this.type === "artist" && image) row = row.icon_mask("circle");
    if (subtitle) row = row.subtitle(subtitle);
    if (accessory) row = row.accessory(accessory);
    if (item.explicit) row = row.tag("E");
    return row.actions(this.actions(item));
  }

  dropdown() {
    return new ListDropdown("Search For")
      .value(this.type)
      .children(TYPES.map(({ value, title }) => new ListDropdownItem(value, title)))
      .on_change((value, cx) => {
        this.type = value;
        this.results = [];
        if (this.query) this.search(cx);
        cx.notify();
      });
  }

  render() {
    if (!this.signed_in) {
      return signInPage((cx) => {
        this.signed_in = true;
        cx.notify();
      });
    }
    const kind = TYPES.find((type) => type.value === this.type).title.toLowerCase();
    const [title, description] = this.error
      ? ["Cannot search Spotify", this.error]
      : this.query
        ? [`No ${kind} found`, "Try other words."]
        : ["Search Spotify", `Type to find ${kind}.`];
    return new List()
      .placeholder(`Search ${kind}…`)
      .loading(this.loading)
      .dropdown(this.dropdown())
      .empty_title(this.loading ? "Searching…" : title)
      .empty_description(this.loading ? `Looking for “${this.query}”` : description)
      .on_query_change((query, cx) => this.typed(query, cx))
      .children(this.results.map((item) => this.row(item)));
  }
}
