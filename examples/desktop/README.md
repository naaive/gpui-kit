# Desktop

A GNOME-style desktop shell prototype for Linux: a top bar on every display and
a dock along the bottom, drawn by GPUI Kit on Wayland layer-shell surfaces.

GPUI is a Wayland client, so the shell runs inside a compositor rather than
being one. Run it from a session of a compositor that implements
`wlr-layer-shell` (sway, Hyprland, niri, KDE Plasma or COSMIC); GNOME's Mutter
does not, and X11 is not supported.

```bash
cargo run -p desktop
```

## What it shows

| Surface | Contents |
| --- | --- |
| Top bar | Workspaces (click to switch), the clock, and network, volume and battery status |
| Date menu | Opens from the clock: today's date and a month calendar |
| System menu | Opens from the status area: a volume slider with mute, the connection and the battery |
| Dock | Favorite applications and other open ones, with a mark for each that has a window; click to focus its window or start it |

Dock favorites are desktop IDs, one per line, in
`$XDG_CONFIG_HOME/gpui-kit-desktop/favorites`. Without that file the dock shows
the installed applications from a built-in list.

## How it is built

| Module | Owns |
| --- | --- |
| `compositor` | Workspaces, open windows and window commands over the sway/i3 IPC socket (`SWAYSOCK`) |
| `system_status` | Battery and network from `/sys`, volume through WirePlumber's `wpctl` |
| `apps` | Desktop entries, a minimal icon theme lookup and `Exec` field codes |
| `top_bar`, `dock` | The layer-shell views |
| `calendar_popup`, `quick_settings`, `popup` | Menus as native `xdg_popup` windows parented to the bar |

The bar's menus cannot be in-window `Popover`s: a layer surface is only as tall
as the bar, so they open as anchored popup windows that the compositor places.

## Limits of the prototype

- **Compositor-specific.** Workspaces and the window list come from sway's IPC.
  Hyprland and niri need their own adapters behind the same `Compositor` model,
  or the shell needs `ext-workspace` and `wlr-foreign-toplevel`, which GPUI does
  not expose.
- **Polling.** Status is read every five seconds instead of following UPower,
  NetworkManager and PipeWire over D-Bus.
- **Outputs at start-up only.** A display connected later gets no bar, and
  every bar lists all workspaces rather than its own output's.
- **No overview, notifications, lock screen or tray.** Live window thumbnails
  need GPUI to show external buffers, the lock screen needs
  `ext-session-lock`, and the tray is a D-Bus service of its own.
- **Dock geometry.** `Root` paints the whole window, so the dock's surface hugs
  its icons with square corners, and a tooltip would be clipped by the surface;
  icons carry an accessible name instead.
- **Popup size is fixed at opening.** Resizing an open popup needs
  `xdg_popup.reposition`, which sway 1.9 does not offer, so the date menu
  always leaves room for a six-week month.
