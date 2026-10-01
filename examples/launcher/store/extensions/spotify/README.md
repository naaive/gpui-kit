# Spotify

Search Spotify, see what is playing, and control playback from the launcher
and the system tray, through the Spotify Web API.

## Commands

- **Search Spotify**: search tracks, albums, artists or playlists as you type
  (the dropdown picks which); Play, Add to Queue, Open in Spotify, Open in
  Browser, Copy Link.
- **Now Playing**: the current track with its artwork; Play/Pause, Next and
  Previous Track, Like, shuffle and repeat. Without an active device it says
  so and offers Open Spotify.
- **Play/Pause**, **Next Track**, **Previous Track**: no page; a HUD names the
  track.
- **Now Playing in Tray**: the track in the tray (its title on macOS, the
  tooltip elsewhere) with playback controls; refreshes every 30 seconds.
- **Sign Out of Spotify**: forgets the tokens.

## Setup

Spotify requires every app to be registered, so you register your own:

1. Open the [Spotify Developer Dashboard](https://developer.spotify.com/dashboard)
   and choose **Create app**. Tick **Web API**.
2. Add the redirect URI `http://127.0.0.1:8973/callback` exactly (use another
   port if you change **Redirect Port** in the preferences).
3. Copy the app's **Client ID** into the extension's **Client ID** preference.
   No client secret is needed: the launcher signs in with PKCE.
4. Open any command and choose **Sign In**; approve the access in the browser.

Controlling playback needs Spotify Premium. While your app is in Spotify's
development mode, add your account under the app's **User Management**.

## Permissions

- Network, `api.spotify.com` only: `GET /v1/search`; `GET`, `PUT` and `POST`
  under `/v1/me/player` (state and playback control); `GET`, `PUT` and
  `DELETE` under `/v1/me/library` and `/v1/me/tracks` (Liked Songs). And
  `POST accounts.spotify.com/api/token`, to sign in and refresh the token.
- Scopes: `user-read-playback-state`, `user-modify-playback-state`,
  `user-read-currently-playing`, `user-library-read`, `user-library-modify`.
  Tokens are kept in the system keychain.
