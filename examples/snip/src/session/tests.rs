//! A whole session through real input: capture from a fake screen, select,
//! annotate, then copy or pin, on GPUI's test platform.

use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Point, TestAppContext, point, px};

use crate::{
    app,
    capture::{FakeCapturer, Frame},
    geometry::{DisplayArea, PhysRect, WindowSnapshot},
    output::clipboard::{ClipboardContent, MemoryClipboard},
    pin,
    scene::Color,
    session,
};

/// The test platform's one display: id 1, 1920×1080.
const DISPLAY: PhysRect = PhysRect::new(0, 0, 1920, 1080);
const WINDOW: PhysRect = PhysRect::new(200, 100, 600, 400);

fn frame() -> Frame {
    let pixels = [40, 80, 120, 255].repeat((DISPLAY.width * DISPLAY.height) as usize);
    Frame::new(DisplayArea::new(DISPLAY, 1.), 1, pixels).unwrap()
}

/// Starts Snip with a fake screen and clipboard, and opens a session.
fn open_session(cx: &mut TestAppContext) -> (AnyWindowHandle, Arc<MemoryClipboard>) {
    let clipboard = Arc::new(MemoryClipboard::default());
    let capturer = Arc::new(FakeCapturer::new(
        vec![frame()],
        vec![WindowSnapshot::new(WINDOW)],
    ));
    cx.update(|cx| {
        gpui_kit::init(cx);
        app::init(cx);
        app::start(
            app::Startup::isolated(capturer, clipboard.clone(), None),
            cx,
        );
        session::start(cx);
    });
    cx.run_until_parked();
    let overlay = cx
        .update(|cx| session::overlay_windows(cx))
        .first()
        .copied()
        .expect("an overlay opens for the display");
    (overlay, clipboard)
}

fn at(x: f32, y: f32) -> Point<gpui_kit::Pixels> {
    point(px(x), px(y))
}

fn with_overlay(
    overlay: AnyWindowHandle,
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App),
) {
    cx.update_window(overlay, |_, window, cx| f(window, cx))
        .unwrap();
    cx.run_until_parked();
}

fn right_click(
    window: &mut gpui_kit::Window,
    position: Point<gpui_kit::Pixels>,
    cx: &mut gpui_kit::App,
) {
    window.render_frame(cx);
    window.dispatch_event(
        gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
            button: gpui_kit::MouseButton::Right,
            position,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
            button: gpui_kit::MouseButton::Right,
            position,
            modifiers: Default::default(),
            click_count: 1,
        }),
        cx,
    );
}

fn annotation_count(cx: &mut TestAppContext) -> usize {
    cx.update(|cx| {
        app::session(cx).map_or(0, |session| {
            session
                .read(cx)
                .state()
                .history()
                .current()
                .annotations()
                .len()
        })
    })
}

#[gpui_kit::test]
fn test_drag_select_and_copy(cx: &mut TestAppContext) {
    let (overlay, clipboard) = open_session(cx);
    with_overlay(overlay, cx, |window, cx| {
        window.drag(at(100., 50.), at(420., 290.), cx);
        window.press("enter", cx);
    });
    assert!(
        cx.update(|cx| app::session(cx)).is_none(),
        "copying ends the session"
    );
    let Some(ClipboardContent::Image(image)) = clipboard.content() else {
        panic!("the selection is on the clipboard");
    };
    assert_eq!(image.dimensions(), (320, 240));
    assert_eq!(image.get_pixel(0, 0).0, [40, 80, 120, 255]);
}

#[gpui_kit::test]
fn test_click_selects_window_under_pointer(cx: &mut TestAppContext) {
    let (overlay, clipboard) = open_session(cx);
    with_overlay(overlay, cx, |window, cx| {
        window.drag(at(300., 200.), at(300., 200.), cx);
        window.press("secondary-c", cx);
    });
    let Some(ClipboardContent::Image(image)) = clipboard.content() else {
        panic!("the window is on the clipboard");
    };
    assert_eq!(
        image.dimensions(),
        (WINDOW.width as u32, WINDOW.height as u32)
    );
}

#[gpui_kit::test]
fn test_draw_undo_and_escape_order(cx: &mut TestAppContext) {
    let (overlay, _) = open_session(cx);
    with_overlay(overlay, cx, |window, cx| {
        window.drag(at(100., 100.), at(500., 400.), cx);
        window.press("r", cx);
        window.drag(at(150., 150.), at(250., 250.), cx);
        window.press("a", cx);
        window.drag(at(300., 300.), at(200., 200.), cx);
    });
    assert_eq!(annotation_count(cx), 2);

    with_overlay(overlay, cx, |window, cx| window.press("secondary-z", cx));
    assert_eq!(annotation_count(cx), 1);
    with_overlay(overlay, cx, |window, cx| {
        window.press("secondary-shift-z", cx)
    });
    assert_eq!(annotation_count(cx), 2);

    // The first Escape puts the arrow tool down, the second leaves.
    with_overlay(overlay, cx, |window, cx| window.press("escape", cx));
    assert!(cx.update(|cx| app::session(cx)).is_some());
    with_overlay(overlay, cx, |window, cx| window.press("escape", cx));
    assert!(cx.update(|cx| app::session(cx)).is_none());
}

#[gpui_kit::test]
fn test_annotations_are_in_the_copy(cx: &mut TestAppContext) {
    let (overlay, clipboard) = open_session(cx);
    with_overlay(overlay, cx, |window, cx| {
        window.drag(at(100., 100.), at(300., 300.), cx);
        window.press("r", cx);
        window.drag(at(120., 120.), at(280., 280.), cx);
        window.press("enter", cx);
    });
    let Some(ClipboardContent::Image(image)) = clipboard.content() else {
        panic!("the selection is on the clipboard");
    };
    let red = crate::scene::PALETTE[0];
    let edge = image.get_pixel(20, 100).0;
    assert_eq!(
        Color::rgb(edge[0], edge[1], edge[2]),
        red,
        "the rectangle's edge is drawn"
    );
    assert_eq!(
        image.get_pixel(100, 100).0,
        [40, 80, 120, 255],
        "inside is clear"
    );
}

#[gpui_kit::test]
fn test_pin_and_close(cx: &mut TestAppContext) {
    let (overlay, _) = open_session(cx);
    with_overlay(overlay, cx, |window, cx| {
        window.drag(at(10., 10.), at(110., 60.), cx);
        window.press("f3", cx);
    });
    let pins = cx.update(|cx| pin::handles(cx));
    assert_eq!(pins.len(), 1, "the selection is pinned");
    cx.update_window(pins[0], |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(
        cx.update(|cx| pin::handles(cx)).is_empty(),
        "Escape closes the pin"
    );
}

#[gpui_kit::test]
fn test_right_click_steps_back(cx: &mut TestAppContext) {
    let (overlay, _) = open_session(cx);
    with_overlay(overlay, cx, |window, cx| {
        window.drag(at(10., 10.), at(110., 60.), cx);
    });
    let has_selection = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            app::session(cx).is_some_and(|session| session.read(cx).state().selection().is_some())
        })
    };
    assert!(has_selection(cx));
    with_overlay(overlay, cx, |window, cx| {
        right_click(window, at(600., 600.), cx)
    });
    assert!(!has_selection(cx), "the selection is dropped");
    assert!(cx.update(|cx| app::session(cx)).is_some());
    with_overlay(overlay, cx, |window, cx| {
        right_click(window, at(600., 600.), cx)
    });
    assert!(
        cx.update(|cx| app::session(cx)).is_none(),
        "then the session ends"
    );
}

fn selection(cx: &mut TestAppContext) -> Option<PhysRect> {
    cx.update(|cx| app::session(cx).and_then(|session| session.read(cx).state().selection()))
}

#[gpui_kit::test]
fn test_text_takes_several_lines(cx: &mut TestAppContext) {
    let (overlay, _) = open_session(cx);
    with_overlay(overlay, cx, |window, cx| {
        window.drag(at(100., 100.), at(500., 400.), cx);
        window.press("t", cx);
        window.drag(at(150., 150.), at(150., 150.), cx);
        window.input("first", cx);
        window.press("enter", cx);
        window.input("second", cx);
    });
    assert_eq!(annotation_count(cx), 0, "Enter starts a new line");
    with_overlay(overlay, cx, |window, cx| {
        window.press("secondary-enter", cx)
    });
    let content = cx.update(|cx| {
        let session = app::session(cx).expect("the session stays open");
        let annotations = session
            .read(cx)
            .state()
            .history()
            .current()
            .annotations()
            .to_vec();
        match annotations
            .first()
            .map(|annotation| annotation.shape().clone())
        {
            Some(crate::scene::Shape::Text { content, .. }) => content.to_string(),
            other => panic!("expected a text annotation, got {other:?}"),
        }
    });
    assert_eq!(content, "first\nsecond");
}

#[gpui_kit::test]
fn test_recalls_the_last_selection(cx: &mut TestAppContext) {
    let (overlay, _) = open_session(cx);
    with_overlay(overlay, cx, |window, cx| {
        window.drag(at(100., 50.), at(420., 290.), cx);
        window.press("enter", cx);
    });
    cx.update(|cx| session::start(cx));
    cx.run_until_parked();
    let overlay = cx
        .update(|cx| session::overlay_windows(cx))
        .first()
        .copied()
        .expect("a second session opens");
    assert_eq!(selection(cx), None);
    with_overlay(overlay, cx, |window, cx| window.press(",", cx));
    assert_eq!(selection(cx), Some(PhysRect::new(100, 50, 320, 240)));
}
