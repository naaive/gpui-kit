//! Pins: images kept above every other window, at the place they were
//! captured from, to compare, copy from or trace over.

mod window;

use std::sync::Arc;

use anyhow::Result;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, KeyBinding, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, point, px, size,
};
use image::RgbaImage;

pub use window::PinWindow;
use window::{
    ClosePin, CopyPin, ResetPinZoom, RotatePinLeft, RotatePinRight, SavePinAs, ZoomPinIn,
    ZoomPinOut,
};

use crate::{
    geometry::{DisplayArea, PhysPoint},
    output::clipboard::{self, ClipboardContent},
    raster,
    shell::hud,
};

pub(crate) const CONTEXT: &str = "SnipPin";

pub fn init(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("escape", ClosePin, context),
        KeyBinding::new("secondary-c", CopyPin, context),
        KeyBinding::new("secondary-s", SavePinAs, context),
        KeyBinding::new("secondary-0", ResetPinZoom, context),
        KeyBinding::new("secondary-=", ZoomPinIn, context),
        KeyBinding::new("secondary--", ZoomPinOut, context),
        KeyBinding::new(",", RotatePinLeft, context),
        KeyBinding::new(".", RotatePinRight, context),
    ]);
    cx.set_global(Pins::default());
}

/// The open pins, for Close all pins.
#[derive(Default)]
struct Pins(Vec<AnyWindowHandle>);

impl Global for Pins {}

/// Where a pin opens.
#[derive(Clone, Copy, Debug)]
pub enum Placement {
    /// Over the place on screen the image was captured from.
    Captured {
        origin: PhysPoint,
        area: DisplayArea,
        native_id: u64,
    },
    /// Centred on the main display, at one image pixel per device pixel.
    Centered,
}

impl Placement {
    pub fn new(origin: PhysPoint, area: DisplayArea, native_id: u64) -> Self {
        Self::Captured {
            origin,
            area,
            native_id,
        }
    }
}

/// Opens a pin showing `image`.
pub fn open(image: Arc<RgbaImage>, placement: Placement, cx: &mut App) -> Result<()> {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let (bounds, display_id, scale) = match placement {
        Placement::Captured {
            origin,
            area,
            native_id,
        } => {
            let display = cx.displays().into_iter().find(|display| {
                crate::session::matches_area(&area, native_id, display.id(), display.bounds())
            });
            match display {
                Some(display) => {
                    let scale = area.scale();
                    let (x, y) = area.to_logical(origin.x as f32, origin.y as f32);
                    let display_origin = display.bounds().origin;
                    (
                        Bounds::new(
                            point(display_origin.x + px(x), display_origin.y + px(y)),
                            size(px(width / scale), px(height / scale)),
                        ),
                        Some(display.id()),
                        scale,
                    )
                }
                None => centered(width, height, cx),
            }
        }
        Placement::Centered => centered(width, height, cx),
    };
    let is_linux = cfg!(target_os = "linux");
    let is_visible = !crate::app::is_offscreen(cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: is_visible,
        show: is_visible,
        kind: if is_linux {
            WindowKind::Normal
        } else {
            WindowKind::PopUp
        },
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("snip".into()),
        window_decorations: is_linux.then_some(gpui_kit::WindowDecorations::Client),
        ..Default::default()
    };
    let (handle, _) = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| PinWindow::new(image, scale, window, cx))
    })?;
    if is_visible {
        handle
            .update(cx, |_, window, _| {
                crate::shell::platform::ensure_shown(window, true);
                window.activate_window();
            })
            .ok();
    }
    cx.global_mut::<Pins>().0.push(handle);
    Ok(())
}

/// Centred on the primary display, at that display's scale once open.
fn centered(
    width: f32,
    height: f32,
    cx: &App,
) -> (Bounds<gpui_kit::Pixels>, Option<gpui_kit::DisplayId>, f32) {
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    (bounds, None, 0.)
}

/// The open pins, oldest first.
#[cfg_attr(not(any(test, feature = "preview")), allow(dead_code))]
pub fn handles(cx: &App) -> Vec<AnyWindowHandle> {
    cx.global::<Pins>().0.clone()
}

pub(crate) fn forget(handle: AnyWindowHandle, cx: &mut App) {
    cx.global_mut::<Pins>().0.retain(|known| *known != handle);
}

pub fn close_all(cx: &mut App) {
    let pins = std::mem::take(&mut cx.global_mut::<Pins>().0);
    for pin in pins {
        pin.update(cx, |_, window, _| window.remove_window()).ok();
    }
}

/// Pins what the clipboard holds: an image as it is, text as a card.
pub fn pin_clipboard(cx: &mut App) {
    let clipboard = clipboard::clipboard(cx);
    let task = cx.background_spawn(async move {
        let content = clipboard.read()?;
        anyhow::Ok(match content {
            Some(ClipboardContent::Image(image)) => Some(image),
            Some(ClipboardContent::Text(text)) => raster::text_card(&text),
            None => None,
        })
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| match result {
            Ok(Some(image)) => {
                if let Err(error) = open(Arc::new(image), Placement::Centered, cx) {
                    hud::show(format!("Couldn’t pin: {error:#}"), cx);
                }
            }
            Ok(None) => hud::show("The clipboard has no image or text to pin", cx),
            Err(error) => hud::show(format!("Couldn’t read the clipboard: {error:#}"), cx),
        });
    })
    .detach();
}
