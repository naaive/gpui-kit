// Signing in to Spotify and calling its Web API.
//
// Spotify signs a desktop app in with the authorization code flow and PKCE;
// the launcher runs the flow (`oauth_authorize`) and keeps the tokens in the
// system keychain. Each user registers their own Spotify app, so the client id
// and the loopback port come from the extension's preferences.
import { launch, oauth_authorize, oauth_refresh, oauth_remove_tokens, oauth_tokens } from "launcher/api";

const PROVIDER = "spotify";
const API = "https://api.spotify.com/v1";
const DEFAULT_PORT = 8973;
const SCOPES = [
  "user-read-playback-state",
  "user-modify-playback-state",
  "user-read-currently-playing",
  "user-library-read",
  "user-library-modify",
];

/** An answer from Spotify that is not a success, with what it means for the user. */
export class SpotifyError extends Error {
  constructor(message, { status = 0, reason = "" } = {}) {
    super(message);
    this.status = status;
    this.reason = reason;
  }

  /** Nothing is playing anywhere: the user has to open Spotify on a device first. */
  get no_device() {
    return this.reason === "NO_ACTIVE_DEVICE" || this.status === 404;
  }

  get signed_out() {
    return this.reason === "SIGNED_OUT";
  }
}

export function redirectPort() {
  const port = Number.parseInt(String(launch().preferences.redirect_port ?? ""), 10);
  return port >= 1024 && port <= 65535 ? port : DEFAULT_PORT;
}

export function redirectUri() {
  return `http://127.0.0.1:${redirectPort()}/callback`;
}

function client() {
  return {
    provider: PROVIDER,
    authorize_url: "https://accounts.spotify.com/authorize",
    token_url: "https://accounts.spotify.com/api/token",
    client_id: String(launch().preferences.client_id ?? "").trim(),
    scope: SCOPES.join(" "),
    redirect_port: redirectPort(),
  };
}

/** The kept tokens, or `null`; never throws (a launcher without a keychain has none). */
function storedTokens() {
  try {
    return oauth_tokens(PROVIDER);
  } catch {
    return null;
  }
}

export function isSignedIn() {
  return storedTokens() !== null;
}

/** Opens Spotify's consent page in the browser and waits for the redirect. */
export async function signIn() {
  return oauth_authorize(client());
}

export function signOut() {
  try {
    oauth_remove_tokens(PROVIDER);
  } catch {
    // Nothing was kept.
  }
}

async function accessToken(force_refresh = false) {
  const tokens = storedTokens();
  if (!tokens) {
    throw new SpotifyError("Sign in to Spotify first", { reason: "SIGNED_OUT" });
  }
  if (!tokens.is_expired && !force_refresh) return tokens.access_token;
  try {
    return (await oauth_refresh(client())).access_token;
  } catch {
    throw new SpotifyError("Your Spotify session expired; sign in again", { reason: "SIGNED_OUT" });
  }
}

function errorFrom(status, body) {
  let reason = "";
  let message = "";
  try {
    const parsed = JSON.parse(body);
    reason = parsed?.error?.reason ?? "";
    message = parsed?.error?.message ?? parsed?.error_description ?? "";
  } catch {
    message = body.trim().slice(0, 200);
  }
  if (reason === "NO_ACTIVE_DEVICE" || (status === 404 && /device/i.test(message))) {
    return new SpotifyError("No active Spotify device. Open Spotify on a device and play something.", {
      status,
      reason: "NO_ACTIVE_DEVICE",
    });
  }
  if (reason === "PREMIUM_REQUIRED") {
    return new SpotifyError("Controlling playback needs Spotify Premium.", { status, reason });
  }
  if (status === 429) {
    return new SpotifyError("Spotify is rate limiting requests; try again in a moment.", { status, reason });
  }
  return new SpotifyError(message || `Spotify answered ${status}.`, { status, reason });
}

/**
 * Calls the Web API: `api("GET", "/me/player")`. Answers the parsed JSON, or
 * `null` for an empty answer (204). Refreshes the token once on a 401.
 */
export async function api(method, path, { query = null, body = null } = {}) {
  const search = query ? `?${new URLSearchParams(query).toString()}` : "";
  const send = async (token) => {
    const options = { method, headers: { Authorization: `Bearer ${token}`, Accept: "application/json" } };
    if (body !== null) {
      options.headers["Content-Type"] = "application/json";
      options.body = JSON.stringify(body);
    } else if (method === "PUT" || method === "POST") {
      // Spotify's player endpoints ask for a length on an empty PUT or POST.
      options.body = "";
    }
    return fetch(`${API}${path}${search}`, options);
  };
  let response = await send(await accessToken());
  if (response.status === 401) response = await send(await accessToken(true));
  const text = await response.text();
  if (!response.ok) throw errorFrom(response.status, text);
  if (!text.trim()) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

/** A user-facing message for anything an `api` call may throw. */
export function describe(error) {
  if (error instanceof SpotifyError) return error.message;
  const text = String(error?.message ?? error);
  return /not allow|capabilit|permission/i.test(text)
    ? "The extension may not reach Spotify; allow its network access."
    : `Cannot reach Spotify: ${text}`;
}

/** The `https://open.spotify.com/…` link for a `spotify:type:id` URI. */
export function webUrl(uri) {
  const [, type, id] = String(uri).split(":");
  return type && id ? `https://open.spotify.com/${type}/${id}` : "https://open.spotify.com";
}

/** The largest image under `max` pixels, or the first, of a Spotify `images` array. */
export function artwork(images, max = 640) {
  const list = (images ?? []).filter((image) => image?.url);
  if (list.length === 0) return null;
  const fitting = list.filter((image) => !image.width || image.width <= max);
  return (fitting[0] ?? list[list.length - 1]).url;
}

export function artistNames(artists) {
  return (artists ?? []).map((artist) => artist.name).join(", ");
}

export function formatDuration(ms) {
  const seconds = Math.max(0, Math.round((ms ?? 0) / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
