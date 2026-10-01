# Snip

A screenshot tool in the manner of Snipaste. Press the capture shortcut to freeze
every display, select a window or drag an area, annotate it, then copy it, save it
or pin it above your other windows. Snip lives in the tray.

```bash
cargo run -p snip
```

The architecture is described in [`docs/SNIP-DESIGN.md`](../../docs/SNIP-DESIGN.md).

## Capturing

| Key | Does |
| --- | --- |
| `F1` (system-wide) | Freezes the screen; change it in Settings |
| click | Selects the window under the pointer |
| drag | Selects an area; drag the handles to resize, the inside to move |
| `←` `↑` `→` `↓`, with `Shift` | Moves the selection by 1 pixel, or 10 |
| `Cmd/Ctrl` + arrows | Grows or shrinks the selection |
| `Cmd/Ctrl-A` | Selects the whole display |
| `R` `E` `A` `L` `P` `M` `X` `T` `N` | Rectangle, ellipse, arrow, line, pen, marker, mosaic, text, step number |
| `[` `]` | Thinner or thicker strokes |
| press on a mark | Picks it: drag to move it, choose a color to recolor it, `Delete` to remove it |
| `Cmd/Ctrl-Z`, `Cmd/Ctrl-Shift-Z` | Undo, redo |
| `C`, `Shift-C` | Copies the color under the magnifier as `#RRGGBB` or `rgb()` |
| `Enter`, `Cmd/Ctrl-C`, double-click | Copies the capture and closes |
| `Cmd/Ctrl-S`, `Cmd/Ctrl-Shift-S` | Saves to the save folder, or asks where |
| `F3`, `Cmd/Ctrl-T` | Pins the capture to the screen |
| `Esc`, right-click | Steps back: the text, the drag, the picked mark, the tool, the selection, then closes |

## Pins

Drag a pin to move it, scroll to zoom, scroll with `Cmd/Ctrl` held to change its
opacity, and double-click or press `Esc` to close it. Its context menu copies,
saves, rotates and closes it. `F3` outside a capture pins whatever image or text
is on the clipboard.

## Command line

`snip capture`, `snip pin-clipboard`, `snip settings` and `snip quit` reach the
running instance, or start one. Where the system has no global shortcuts for
applications (Wayland), bind `snip capture` in your desktop's keyboard settings.

## Previewing the interface

```bash
cargo run -p snip --features preview -- --render-preview target/snip-preview
```

captures the screen, plays a session on hidden windows and writes what the
overlay, a pin and the settings window show, in light and dark themes, as PNG
files. Nothing appears on screen and the clipboard is left alone.

## Platforms

Windows captures with DXGI Desktop Duplication (HDR desktops are mapped to SDR),
macOS with ScreenCaptureKit, X11 from the root window, and Wayland through the
screenshot portal. See the design document for what each platform can and can't
do.
