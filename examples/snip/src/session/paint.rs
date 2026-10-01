//! Drawing annotations in an overlay. Annotations arrive as raster tiles
//! made by the same renderer export uses; the overlay draws no GPUI paths,
//! whose rendering costs a window-sized multisampled pass per frame.

use gpui_kit::{
    BorderStyle, Bounds, Corners, Hsla, Pixels, Point, Rgba, Window, fill, outline, point, px,
};

use super::TileSprite;
use crate::{
    geometry::DisplayArea,
    scene::{Annotation, Color, ScenePoint, Shape},
};

/// An annotation color as GPUI paints it.
pub fn hsla(color: Color) -> Hsla {
    Rgba {
        r: color.r as f32 / 255.,
        g: color.g as f32 / 255.,
        b: color.b as f32 / 255.,
        a: color.a as f32 / 255.,
    }
    .into()
}

/// Paints a rectangle being drawn as a quad: its border is the stroke,
/// centred on the edges, with square corners as the exported miter joins.
/// Other drafts are raster tiles (see `CaptureSession::refresh_draft`).
pub fn paint_rectangle_draft(
    draft: &Annotation,
    area: &DisplayArea,
    origin: Point<Pixels>,
    window: &mut Window,
) {
    let Shape::Rectangle { from, to } = draft.shape() else {
        return;
    };
    let style = draft.style();
    let color = hsla(style.color());
    let at = |scene: ScenePoint| -> Point<Pixels> {
        let (x, y) = area.to_logical(scene.x, scene.y);
        point(origin.x + px(x), origin.y + px(y))
    };
    let bounds = Bounds::from_corners(
        at(ScenePoint::new(from.x.min(to.x), from.y.min(to.y))),
        at(ScenePoint::new(from.x.max(to.x), from.y.max(to.y))),
    );
    if style.is_filled() {
        window.paint_quad(fill(bounds, color));
        return;
    }
    let width = px(area.to_logical_length(style.stroke_width()));
    window.paint_quad(
        outline(bounds.dilate(width / 2.), color, BorderStyle::Solid).border_widths(width),
    );
}

/// Paints a raster tile at its desktop position, shifted by `shift`
/// physical pixels.
pub fn paint_sprite(
    sprite: &TileSprite,
    shift: (f32, f32),
    area: &DisplayArea,
    origin: Point<Pixels>,
    window: &mut Window,
) {
    let bounds = super::logical_bounds(area, sprite.tile().bounds());
    let (dx, dy) = (
        area.to_logical_length(shift.0),
        area.to_logical_length(shift.1),
    );
    let bounds = Bounds::new(
        point(
            origin.x + bounds.origin.x + px(dx),
            origin.y + bounds.origin.y + px(dy),
        ),
        bounds.size,
    );
    window
        .paint_image(
            bounds,
            bounds,
            Corners::default(),
            sprite.image().clone(),
            0,
            false,
        )
        .ok();
}
