//! One pinned image in a borderless, topmost window.
//!
//! Drag anywhere to move it, scroll to zoom, hold the secondary modifier
//! while scrolling to change its opacity, double-click or press Escape to
//! close it. Every command is also in its context menu.

use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, menu::ContextMenuExt as _};
use gpui_kit::{
    Action, App, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Render, RenderImage, ScrollWheelEvent,
    Size, Styled as _, Window, actions, div, img, prelude::FluentBuilder as _, px, size,
};
use image::RgbaImage;
use serde::Deserialize;

use super::CONTEXT;
use crate::{output, session::render_image};

actions!(
    snip_pin,
    [
        ClosePin,
        CloseAllPins,
        CopyPin,
        SavePinAs,
        ResetPinZoom,
        ZoomPinIn,
        ZoomPinOut,
        RotatePinLeft,
        RotatePinRight,
    ]
);

/// Sets a pin's opacity, in percent.
#[derive(Action, Clone, PartialEq, Deserialize)]
#[action(namespace = snip_pin, no_json)]
pub struct SetPinOpacity(pub u8);

const ZOOM_STEP: f32 = 1.1;
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 8.;
const OPACITY_STEP: f32 = 0.1;
const MIN_OPACITY: f32 = 0.1;

pub struct PinWindow {
    /// The image as captured, before rotation.
    image: Arc<RgbaImage>,
    /// The image as shown, rotated.
    sprite: Arc<RenderImage>,
    /// Image pixels per logical pixel at 100%: the scale of the display it
    /// was captured on, so a pin opens at exactly the captured size.
    scale: f32,
    zoom: f32,
    opacity: f32,
    /// Whether the window itself is faded to `opacity`, rather than the
    /// image drawn in it.
    is_window_faded: bool,
    quarter_turns: u8,
    focus_handle: FocusHandle,
}

impl PinWindow {
    /// A pin of `image`; a `scale` of zero takes the window's own.
    pub fn new(
        image: Arc<RgbaImage>,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let scale = if scale > 0. {
            scale
        } else {
            window.scale_factor()
        };
        let this = Self {
            sprite: render_image(&image),
            image,
            scale,
            zoom: 1.,
            opacity: 1.,
            is_window_faded: false,
            quarter_turns: 0,
            focus_handle,
        };
        this.fit_window(window);
        this
    }

    /// The window size for the current zoom and rotation.
    fn logical_size(&self) -> Size<Pixels> {
        let (width, height) = if self.quarter_turns % 2 == 0 {
            (self.image.width(), self.image.height())
        } else {
            (self.image.height(), self.image.width())
        };
        let factor = self.zoom / self.scale;
        size(px(width as f32 * factor), px(height as f32 * factor))
    }

    fn fit_window(&self, window: &mut Window) {
        let target = self.logical_size();
        if window.bounds().size != target {
            window.resize(target);
        }
    }

    fn set_zoom(&mut self, zoom: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.fit_window(window);
        cx.notify();
    }

    fn set_opacity(&mut self, opacity: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.opacity = opacity.clamp(MIN_OPACITY, 1.);
        self.is_window_faded = crate::shell::platform::set_window_opacity(window, self.opacity);
        cx.notify();
    }

    fn rotate(&mut self, clockwise: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.quarter_turns = if clockwise {
            (self.quarter_turns + 1) % 4
        } else {
            (self.quarter_turns + 3) % 4
        };
        self.sprite = render_image(&self.rotated());
        self.fit_window(window);
        cx.notify();
    }

    /// The image as shown, for copying and saving.
    fn rotated(&self) -> RgbaImage {
        match self.quarter_turns {
            1 => image::imageops::rotate90(&*self.image),
            2 => image::imageops::rotate180(&*self.image),
            3 => image::imageops::rotate270(&*self.image),
            _ => (*self.image).clone(),
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        super::forget(window.window_handle(), cx);
        window.remove_window();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if event.click_count == 2 {
            self.close(window, cx);
        } else {
            window.start_window_move();
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(window.line_height()).y;
        if delta == px(0.) {
            return;
        }
        let up = delta > px(0.);
        if event.modifiers.secondary() {
            let step = if up { OPACITY_STEP } else { -OPACITY_STEP };
            self.set_opacity(self.opacity + step, window, cx);
        } else {
            let zoom = if up {
                self.zoom * ZOOM_STEP
            } else {
                self.zoom / ZOOM_STEP
            };
            self.set_zoom(zoom, window, cx);
        }
    }
}

impl Focusable for PinWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PinWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_focused = self.focus_handle.is_focused(window);
        let opacity = (self.opacity * 100.).round() as u8;
        div()
            .id("pin")
            .track_focus(&self.focus_handle)
            .key_context(CONTEXT)
            .size_full()
            .relative()
            .on_action(cx.listener(|this, _: &ClosePin, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|_, _: &CloseAllPins, _, cx| super::close_all(cx)))
            .on_action(cx.listener(|this, _: &CopyPin, _, cx| {
                output::copy_image(Arc::new(this.rotated()), cx);
            }))
            .on_action(cx.listener(|this, _: &SavePinAs, _, cx| {
                output::save_image_as(Arc::new(this.rotated()), cx);
            }))
            .on_action(
                cx.listener(|this, _: &ResetPinZoom, window, cx| this.set_zoom(1., window, cx)),
            )
            .on_action(cx.listener(|this, _: &ZoomPinIn, window, cx| {
                this.set_zoom(this.zoom * ZOOM_STEP, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomPinOut, window, cx| {
                this.set_zoom(this.zoom / ZOOM_STEP, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &RotatePinLeft, window, cx| this.rotate(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &RotatePinRight, window, cx| this.rotate(true, window, cx)),
            )
            .on_action(cx.listener(|this, action: &SetPinOpacity, window, cx| {
                this.set_opacity(action.0 as f32 / 100., window, cx);
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                img(self.sprite.clone())
                    .size_full()
                    .when(!self.is_window_faded, |this| this.opacity(self.opacity)),
            )
            // A frame above the image, which would cover a border of the
            // window's own; it marks the pin the keyboard talks to.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .border_1()
                    .border_color(if is_focused {
                        theme.selection.opacity(1.)
                    } else {
                        theme.border
                    }),
            )
            .when(self.zoom != 1., |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom_1()
                        .right_1()
                        .px_1()
                        .rounded(theme.radius)
                        .bg(theme.popover)
                        .text_color(theme.popover_foreground)
                        .text_xs()
                        .child(format!("{}%", (self.zoom * 100.).round())),
                )
            })
            .context_menu(move |menu, window, cx| {
                menu.menu("Copy", Box::new(CopyPin))
                    .menu("Save as…", Box::new(SavePinAs))
                    .separator()
                    .menu("Rotate left", Box::new(RotatePinLeft))
                    .menu("Rotate right", Box::new(RotatePinRight))
                    .menu("Zoom to 100%", Box::new(ResetPinZoom))
                    .submenu("Opacity", window, cx, move |menu, _, _| {
                        [100u8, 80, 60, 40, 20]
                            .into_iter()
                            .fold(menu, |menu, percent| {
                                menu.menu_with_check(
                                    format!("{percent}%"),
                                    opacity == percent,
                                    Box::new(SetPinOpacity(percent)),
                                )
                            })
                    })
                    .separator()
                    .menu("Close", Box::new(ClosePin))
                    .menu("Close all pins", Box::new(CloseAllPins))
            })
    }
}
