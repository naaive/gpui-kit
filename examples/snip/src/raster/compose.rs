//! The exported image: the selected part of a frame with every annotation
//! drawn on it, on the CPU.

use anyhow::{Context as _, Result};
use image::RgbaImage;
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint as SkiaPaint, PathBuilder, Pixmap, PixmapPaint, Stroke,
    Transform,
};

use std::sync::Arc;

use super::tiles::{self, Tile, TileCache};
use crate::{
    capture::Frame,
    geometry::PhysRect,
    scene::{Annotation, Figure, Paint, PathOp, Scene, bounds, outline},
};

/// Renders `selection` of `frame` with `scene` on top.
pub fn compose(
    frame: &Frame,
    selection: PhysRect,
    scene: &Scene,
    tiles: &mut TileCache,
) -> Result<RgbaImage> {
    let base = frame
        .crop(selection)
        .context("the selection isn't on the captured display")?;
    let (width, height) = base.dimensions();
    // The frame is opaque, so its straight RGBA is already premultiplied.
    let mut pixmap = Pixmap::from_vec(
        base.into_raw(),
        tiny_skia::IntSize::from_wh(width, height).context("empty selection")?,
    )
    .context("cannot hold the selection")?;
    let to_selection = Transform::from_translate(-selection.x as f32, -selection.y as f32);

    for annotation in scene.annotations() {
        for figure in outline(annotation) {
            draw_figure(&mut pixmap, &figure, to_selection);
        }
        if let Some(tile) = tiles.get(annotation, Some(frame)) {
            let image = tile.image();
            let Some(tile_pixmap) = premultiplied(image) else {
                continue;
            };
            pixmap.draw_pixmap(
                tile.x() - selection.x,
                tile.y() - selection.y,
                tile_pixmap.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }

    let mut pixels = pixmap.take();
    // Everything drawn sits on an opaque base, so the result is opaque and
    // needs no un-premultiplying; alpha is forced to cover rounding.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    RgbaImage::from_raw(width, height, pixels).context("cannot build the image")
}

/// Everything one annotation draws, as a single tile: its figures
/// rasterized exactly as export draws them, with its text, number or mosaic
/// on top.
///
/// The overlay shows annotations as these tiles rather than as GPUI paths:
/// each path batch makes GPUI's Windows renderer clear and resolve a
/// window-sized multisampled texture, which at 4K costs tens of
/// milliseconds a frame on integrated graphics.
pub fn sprite(annotation: &Annotation, frame: Option<&Frame>) -> Option<Tile> {
    let figures = outline(annotation);
    let extra = tiles::tile(annotation, frame);
    if figures.is_empty() {
        return extra;
    }
    // Miter corners reach past half the stroke; antialiasing a pixel more.
    let pad = annotation.style().stroke_width() + 2.;
    let (min, max) = bounds(annotation);
    // An arrow that hasn't left its start yet outlines nothing.
    if !(min.x <= max.x && min.y <= max.y) {
        return extra;
    }
    let mut area = PhysRect::from_edges(
        (min.x - pad).floor() as i32,
        (min.y - pad).floor() as i32,
        (max.x + pad).ceil() as i32,
        (max.y + pad).ceil() as i32,
    );
    if let Some(extra) = &extra {
        let other = extra.bounds();
        area = PhysRect::from_edges(
            area.x.min(other.x),
            area.y.min(other.y),
            area.right().max(other.right()),
            area.bottom().max(other.bottom()),
        );
    }
    let mut pixmap = Pixmap::new(area.width as u32, area.height as u32)?;
    let to_area = Transform::from_translate(-area.x as f32, -area.y as f32);
    for figure in &figures {
        draw_figure(&mut pixmap, figure, to_area);
    }
    if let Some(extra) = &extra
        && let Some(extra_pixmap) = premultiplied(extra.image())
    {
        pixmap.draw_pixmap(
            extra.x() - area.x,
            extra.y() - area.y,
            extra_pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    let mut pixels = pixmap.take();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u16;
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u16 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    let image = RgbaImage::from_raw(area.width as u32, area.height as u32, pixels)?;
    Some(Tile::new(area.x, area.y, Arc::new(image)))
}

fn draw_figure(pixmap: &mut Pixmap, figure: &Figure, transform: Transform) {
    let mut path = PathBuilder::new();
    for op in figure.ops() {
        match *op {
            PathOp::MoveTo(point) => path.move_to(point.x, point.y),
            PathOp::LineTo(point) => path.line_to(point.x, point.y),
            PathOp::CubicTo(first, second, to) => {
                path.cubic_to(first.x, first.y, second.x, second.y, to.x, to.y)
            }
            PathOp::Close => path.close(),
        }
    }
    let Some(path) = path.finish() else {
        return;
    };
    let color = figure.color();
    let mut paint = SkiaPaint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    match figure.paint() {
        Paint::Fill => {
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
        }
        Paint::Stroke { width, is_round } => {
            let stroke = Stroke {
                width,
                line_cap: if is_round {
                    LineCap::Round
                } else {
                    LineCap::Butt
                },
                line_join: if is_round {
                    LineJoin::Round
                } else {
                    LineJoin::Miter
                },
                ..Stroke::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, transform, None);
        }
    }
}

/// A straight-alpha tile as the premultiplied pixmap tiny-skia blends.
fn premultiplied(image: &RgbaImage) -> Option<Pixmap> {
    let mut pixels = image.as_raw().clone();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u16;
        for channel in &mut pixel[..3] {
            *channel = ((*channel as u16 * alpha + 127) / 255) as u8;
        }
    }
    Pixmap::from_vec(
        pixels,
        tiny_skia::IntSize::from_wh(image.width(), image.height())?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        capture::FakeCapturer,
        scene::{Annotation, AnnotationId, Color, ScenePoint, Shape, Style},
    };

    const WHITE: Color = Color::rgb(255, 255, 255);
    const RED: Color = Color::rgb(255, 0, 0);

    fn frame() -> Frame {
        FakeCapturer::solid_frame(PhysRect::new(100, 100, 64, 64), 1., WHITE)
    }

    fn scene(shape: Shape, style: Style) -> Scene {
        Scene::default().with(Annotation::new(AnnotationId(1), shape, style))
    }

    #[test]
    fn test_compose_crops_to_selection() {
        let image = compose(
            &frame(),
            PhysRect::new(110, 120, 10, 5),
            &Scene::default(),
            &mut TileCache::default(),
        )
        .unwrap();
        assert_eq!(image.dimensions(), (10, 5));
        assert_eq!(image.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    #[test]
    fn test_compose_draws_in_desktop_coordinates() {
        let style = Style::default().with_color(RED).with_filled(true);
        let scene = scene(
            Shape::Rectangle {
                from: ScenePoint::new(120., 120.),
                to: ScenePoint::new(130., 130.),
            },
            style,
        );
        let image = compose(
            &frame(),
            PhysRect::new(110, 110, 40, 40),
            &scene,
            &mut TileCache::default(),
        )
        .unwrap();
        assert_eq!(
            image.get_pixel(15, 15).0,
            [255, 0, 0, 255],
            "inside the box"
        );
        assert_eq!(image.get_pixel(5, 5).0, [255, 255, 255, 255], "outside it");
    }

    #[test]
    fn test_compose_outlined_rectangle_leaves_inside_clear() {
        let style = Style::default().with_color(RED).with_stroke_width(2.);
        let scene = scene(
            Shape::Rectangle {
                from: ScenePoint::new(110., 110.),
                to: ScenePoint::new(150., 150.),
            },
            style,
        );
        let image = compose(
            &frame(),
            PhysRect::new(100, 100, 64, 64),
            &scene,
            &mut TileCache::default(),
        )
        .unwrap();
        assert_eq!(image.get_pixel(10, 30).0, [255, 0, 0, 255], "on the edge");
        assert_eq!(image.get_pixel(30, 30).0, [255, 255, 255, 255], "inside");
    }

    #[test]
    fn test_sprite_matches_export() {
        let style = Style::default().with_color(RED).with_stroke_width(4.);
        let annotation = Annotation::new(
            AnnotationId(1),
            Shape::Ellipse {
                from: ScenePoint::new(110., 110.),
                to: ScenePoint::new(150., 140.),
            },
            style,
        );
        let sprite = sprite(&annotation, None).unwrap();
        let selection = PhysRect::new(100, 100, 64, 64);
        let exported = compose(
            &frame(),
            selection,
            &Scene::default().with(annotation),
            &mut TileCache::default(),
        )
        .unwrap();
        // Where the sprite is opaque, export drew the same color.
        let image = sprite.image();
        let (x, y) = (150 - sprite.x(), 125 - sprite.y());
        assert_eq!(image.get_pixel(x as u32, y as u32).0[3], 255);
        assert_eq!(
            exported
                .get_pixel((150 - selection.x) as u32, (125 - selection.y) as u32)
                .0,
            image.get_pixel(x as u32, y as u32).0
        );
        assert!(sprite.bounds().x <= 106 && sprite.bounds().right() >= 154);
    }

    #[test]
    fn test_compose_marker_is_translucent() {
        let style = Style::default().with_color(RED).with_stroke_width(4.);
        let scene = scene(
            Shape::Marker {
                points: [ScenePoint::new(105., 130.), ScenePoint::new(155., 130.)].into(),
            },
            style,
        );
        let image = compose(
            &frame(),
            PhysRect::new(100, 100, 64, 64),
            &scene,
            &mut TileCache::default(),
        )
        .unwrap();
        let [r, g, b, _] = image.get_pixel(30, 30).0;
        assert_eq!(r, 255);
        assert!(g > 100 && g < 200 && g == b, "white shows through: {g}");
    }
}
