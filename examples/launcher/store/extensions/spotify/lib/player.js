// Spotify's player and library endpoints, as small functions the commands
// share. Every one throws a `SpotifyError` the caller shows to the user.
import { api, artistNames, artwork, SpotifyError } from "./spotify.js";

const REPEAT_NEXT = { off: "context", context: "track", track: "off" };

/** The playback state, or `null` when nothing is playing on any device. */
export async function playback() {
  const state = await api("GET", "/me/player", { query: { additional_types: "track,episode" } });
  return state && state.device ? state : null;
}

/** What `playback()` says is playing, in the shape the pages show. */
export function nowPlaying(state) {
  const item = state?.item;
  if (!item) return null;
  const episode = item.type === "episode";
  return {
    id: item.id,
    uri: item.uri,
    name: item.name,
    artists: episode ? item.show?.name ?? "" : artistNames(item.artists),
    album: episode ? item.show?.publisher ?? "" : item.album?.name ?? "",
    image: artwork(episode ? item.images ?? item.show?.images : item.album?.images),
    duration_ms: item.duration_ms ?? 0,
    progress_ms: state.progress_ms ?? 0,
    is_playing: Boolean(state.is_playing),
    shuffle: Boolean(state.shuffle_state),
    repeat: state.repeat_state ?? "off",
    device: state.device?.name ?? "",
    volume: state.device?.volume_percent ?? null,
    url: item.external_urls?.spotify ?? "",
    episode,
  };
}

/** Plays a list of track URIs, or a `context_uri` (album, playlist, artist); nothing resumes. */
export async function play({ uris = null, context_uri = null } = {}) {
  const body = uris ? { uris } : context_uri ? { context_uri } : null;
  await api("PUT", "/me/player/play", { body });
}

export async function pause() {
  await api("PUT", "/me/player/pause");
}

/** Pauses what plays, or resumes what is paused; answers whether it now plays. */
export async function togglePlayback() {
  const state = await playback();
  if (!state) throw new SpotifyError("No active Spotify device", { status: 404, reason: "NO_ACTIVE_DEVICE" });
  if (state.is_playing) {
    await pause();
    return { playing: false, track: nowPlaying(state) };
  }
  await play();
  return { playing: true, track: nowPlaying(state) };
}

export async function next() {
  await api("POST", "/me/player/next");
}

export async function previous() {
  await api("POST", "/me/player/previous");
}

export async function addToQueue(uri) {
  await api("POST", "/me/player/queue", { query: { uri } });
}

export async function setShuffle(on) {
  await api("PUT", "/me/player/shuffle", { query: { state: String(Boolean(on)) } });
}

/** Moves repeat from off to the whole context, to one track, and back; answers the new mode. */
export async function cycleRepeat(current) {
  const mode = REPEAT_NEXT[current] ?? "off";
  await api("PUT", "/me/player/repeat", { query: { state: mode } });
  return mode;
}

/**
 * Saves a track to Liked Songs. Spotify has been moving apps to the
 * `/me/library` endpoints; an account still on the older `/me/tracks` ones
 * answers 404 or 410 there, and gets those instead.
 */
export async function like(track) {
  try {
    await api("PUT", "/me/library", { query: { uris: track.uri } });
  } catch (error) {
    if (!(error instanceof SpotifyError) || ![400, 404, 410].includes(error.status)) throw error;
    await api("PUT", "/me/tracks", { query: { ids: track.id } });
  }
}

export async function unlike(track) {
  try {
    await api("DELETE", "/me/library", { query: { uris: track.uri } });
  } catch (error) {
    if (!(error instanceof SpotifyError) || ![400, 404, 410].includes(error.status)) throw error;
    await api("DELETE", "/me/tracks", { query: { ids: track.id } });
  }
}

/** Whether a track is in Liked Songs; `false` when Spotify will not say. */
export async function isLiked(track) {
  try {
    const answer = await api("GET", "/me/library/contains", { query: { uris: track.uri } });
    return Array.isArray(answer) && answer[0] === true;
  } catch {
    try {
      const answer = await api("GET", "/me/tracks/contains", { query: { ids: track.id } });
      return Array.isArray(answer) && answer[0] === true;
    } catch {
      return false;
    }
  }
}

/** Waits a moment for Spotify to move on, then answers what plays now. */
export async function settledTrack(cx, previous_id = null) {
  for (let attempt = 0; attempt < 4; attempt += 1) {
    await cx.sleep(400);
    try {
      const track = nowPlaying(await playback());
      if (track && track.id !== previous_id) return track;
    } catch {
      return null;
    }
  }
  return null;
}
