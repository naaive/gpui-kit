//! Menus that drop from a shell surface.
//!
//! A layer-shell bar is only as tall as its row of controls, so an in-window
//! `Popover` would be clipped. These open as native `xdg_popup` windows
//! parented to the bar's layer surface, and the compositor places them.

use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::popup::{PopupAnchor, PopupConstraintAdjustment, PopupGravity, PopupOptions};
use gpui_kit::{
    AnyWindowHandle, App, Bounds, Div, Entity, Pixels, Render, Size, Styled as _, Window,
    WindowBounds, WindowKind, WindowOptions, point, px,
};

/// Opens `build`'s view in a popup that drops below `anchor_rect`, a
/// rectangle in `parent`'s coordinates.
///
/// Call it while the mouse button that opens the popup is still down: the
/// popup takes an input grab, which the compositor grants only during a press.
pub fn open_popup<V: Render>(
    parent: &Window,
    anchor_rect: Bounds<Pixels>,
    size: Size<Pixels>,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Option<(AnyWindowHandle, Entity<V>)> {
    let options = WindowOptions {
        titlebar: None,
        focus: true,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size,
        })),
        kind: WindowKind::AnchoredPopup(PopupOptions {
            parent: parent.window_handle(),
            anchor_rect,
            anchor: PopupAnchor::Bottom,
            gravity: PopupGravity::Bottom,
            constraint_adjustment: PopupConstraintAdjustment::SLIDE_X
                | PopupConstraintAdjustment::FLIP_Y,
            offset: point(px(0.), px(0.)),
            grab: true,
        }),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, build) {
        Ok(opened) => Some(opened),
        Err(err) => {
            eprintln!("desktop: couldn't open popup: {err:#}");
            None
        }
    }
}

/// Closes a popup this shell opened. Returns `false` if it was already gone,
/// for example because the compositor dismissed it on an outside click.
pub fn close_popup(handle: AnyWindowHandle, cx: &mut App) -> bool {
    handle
        .update(cx, |_, window, _| window.remove_window())
        .is_ok()
}

/// The popup's visible surface, filling the popup window.
///
/// Size the popup for its content when opening it. Resizing an open popup
/// needs `xdg_popup.reposition` (xdg-shell 3), which not every compositor
/// offers; sway 1.9 ignores it.
pub fn popup_surface(cx: &App) -> Div {
    v_flex()
        .size_full()
        .bg(cx.theme().popover)
        .text_color(cx.theme().popover_foreground)
        .border_1()
        .border_color(cx.theme().border)
}
