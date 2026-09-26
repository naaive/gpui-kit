//! The HUD: a short confirmation shown after the launcher hides, such as
//! "Copied to clipboard", in a window of its own because the launcher's is
//! already gone.

use std::time::Duration;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Task, Window, WindowBounds, WindowKind,
    WindowOptions,
    component::{ActiveTheme as _, h_flex},
    font, point, px, size,
};

/// Long enough to read a few words, short enough not to linger.
const DURATION: Duration = Duration::from_millis(1600);
const HEIGHT: f32 = 36.;
const HORIZONTAL_PADDING: f32 = 16.;
/// Distance from the bottom of the display's visible area.
const BOTTOM_OFFSET: f32 = 96.;

/// The HUD on screen, if any; a new message replaces it.
struct ShownHud {
    window: AnyWindowHandle,
    _close: Task<()>,
}

#[derive(Default)]
struct HudState(Option<ShownHud>);

impl Global for HudState {}

struct Hud {
    text: SharedString,
}

impl Render for Hud {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .size_full()
            .justify_center()
            .px(px(HORIZONTAL_PADDING))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .child(self.text.clone())
    }
}

pub(super) fn show(text: SharedString, cx: &mut App) {
    close(cx);
    let width = text_width(&text, cx) + 2. * HORIZONTAL_PADDING + 2.;
    let hud_size = size(px(width.clamp(120., 480.)), px(HEIGHT));
    let Some(display) = cx.primary_display() else {
        return;
    };
    let visible = display.visible_bounds();
    let origin = point(
        visible.center().x - hud_size.width / 2.,
        visible.bottom() - px(BOTTOM_OFFSET) - hud_size.height,
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, hud_size))),
        titlebar: None,
        // The HUD confirms what happened; it must not take focus from the
        // application the user returned to.
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display.id()),
        app_id: Some("gpui-kit-launcher".into()),
        ..Default::default()
    };
    let window = match gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Hud { text })) {
        Ok((window, _)) => window,
        Err(error) => {
            tracing::warn!("cannot show the HUD: {error:#}");
            return;
        }
    };
    let executor = cx.background_executor().clone();
    let close_later = cx.spawn(async move |cx: &mut AsyncApp| {
        executor.timer(DURATION).await;
        cx.update(close);
    });
    cx.set_global(HudState(Some(ShownHud {
        window,
        _close: close_later,
    })));
}

fn close(cx: &mut App) {
    let shown = cx.default_global::<HudState>().0.take();
    if let Some(shown) = shown {
        shown
            .window
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

/// The width of one line of `text` in the theme's interface font, so the
/// window fits its message: a window's size is fixed when it opens.
fn text_width(text: &str, cx: &App) -> f32 {
    let theme = cx.theme();
    let text_system = cx.text_system();
    let font_id = text_system.resolve_font(&font(theme.font_family.clone()));
    text.chars()
        .filter_map(|ch| text_system.advance(font_id, theme.font_size, ch).ok())
        .map(|advance| f32::from(advance.width))
        .sum()
}
