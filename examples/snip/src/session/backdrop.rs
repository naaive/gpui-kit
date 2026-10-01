//! The frozen frame of one display, in a window under its overlay.
//!
//! The frame never changes during a session, so this window is drawn once;
//! the overlay above it, transparent where nothing is drawn, is the one that
//! redraws as the pointer moves.

use std::sync::Arc;

use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, RenderImage, Styled as _, Window, canvas, div,
};

pub struct Backdrop {
    image: Arc<RenderImage>,
}

impl Backdrop {
    pub fn new(image: Arc<RenderImage>) -> Self {
        Self { image }
    }
}

impl Render for Backdrop {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let image = self.image.clone();
        div().size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    tracing::debug!("backdrop paints {bounds:?}");
                    if let Err(error) =
                        window.paint_image(bounds, bounds, Default::default(), image, 0, false)
                    {
                        tracing::error!("cannot draw the frozen screen: {error:#}");
                    }
                },
            )
            .size_full(),
        )
    }
}
