//! The visible windows, frontmost first, for automatic selection.
//!
//! The window server lists on-screen windows front to back. Only ordinary
//! application windows (layer 0) are kept: the menu bar, the Dock, menus and
//! overlays sit on higher layers. macOS doesn't expose a window's controls to
//! other processes without Accessibility access, so snapshots have no parts.

use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGRect,
};
use objc2_core_graphics::{
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer,
};

use super::to_physical_rect;
use crate::geometry::WindowSnapshot;

/// The window layer of ordinary application windows.
const NORMAL_WINDOW_LAYER: i32 = 0;

/// The windows on screen, frontmost first, in desktop pixels at `scale`
/// pixels per point.
pub fn snapshots(scale: f64) -> Vec<WindowSnapshot> {
    let Some(windows) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        tracing::warn!("the window server listed no windows");
        return Vec::new();
    };
    // Each entry is a dictionary of window properties keyed by name.
    let windows: CFRetained<CFArray<CFDictionary<CFString, CFType>>> =
        unsafe { CFRetained::cast_unchecked(windows) };
    windows
        .iter()
        .filter(|window| is_selectable(window))
        .filter_map(|window| {
            let frame = to_physical_rect(bounds(&window)?, scale);
            (!frame.is_empty()).then(|| WindowSnapshot::new(frame))
        })
        .collect()
}

/// Whether a window is an ordinary one that can be seen: on the normal
/// layer and not fully transparent.
fn is_selectable(window: &CFDictionary<CFString, CFType>) -> bool {
    let layer = number(window, unsafe { kCGWindowLayer }).and_then(|layer| layer.as_i32());
    let alpha = number(window, unsafe { kCGWindowAlpha }).and_then(|alpha| alpha.as_f64());
    layer == Some(NORMAL_WINDOW_LAYER) && alpha.is_none_or(|alpha| alpha > 0.)
}

/// The window's frame in global points, origin at the primary display's top
/// left.
fn bounds(window: &CFDictionary<CFString, CFType>) -> Option<CGRect> {
    let bounds = window
        .get(unsafe { kCGWindowBounds })?
        .downcast::<CFDictionary>()
        .ok()?;
    let mut rect = CGRect::default();
    unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut rect) }.then_some(rect)
}

fn number(window: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<CFRetained<CFNumber>> {
    window.get(key)?.downcast::<CFNumber>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lists what automatic selection would offer, frontmost first. Run by
    /// hand with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn smoke_list_windows() {
        for snapshot in snapshots(2.) {
            println!("{snapshot:?}");
        }
    }
}
