//! Vector outlines of annotations, shared by both renderers.
//!
//! The overlay draws these with GPUI paths for live preview, and export
//! draws the very same figures with tiny-skia. Keeping one source of
//! geometry is what makes the saved image match what was on screen; text,
//! step numbers and mosaic, which GPU paths can't reproduce exactly, are
//! raster tiles instead (see `raster::tiles`).

use super::annotation::{Annotation, Color, ScenePoint, Shape};

/// One segment command of a figure's path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathOp {
    MoveTo(ScenePoint),
    LineTo(ScenePoint),
    CubicTo(ScenePoint, ScenePoint, ScenePoint),
    Close,
}

/// How a figure's path is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    /// Round caps and joins suit freehand and lines; boxes keep sharp corners.
    Stroke {
        width: f32,
        is_round: bool,
    },
    Fill,
}

/// A path with its paint, the unit both renderers draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    ops: Vec<PathOp>,
    paint: Paint,
    color: Color,
}

impl Figure {
    pub fn ops(&self) -> &[PathOp] {
        &self.ops
    }

    pub fn paint(&self) -> Paint {
        self.paint
    }

    pub fn color(&self) -> Color {
        self.color
    }
}

/// How opaque the marker is, out of 255.
pub const MARKER_ALPHA: u8 = 102;

/// The step badge radius for a font size, in the same pixels.
pub fn step_radius(font_size: f32) -> f32 {
    font_size * 0.85
}

/// The marker's stroke width for a style's stroke width.
pub fn marker_width(stroke_width: f32) -> f32 {
    (stroke_width * 4.).max(12.)
}

/// The mosaic brush width for a style's stroke width.
pub fn mosaic_width(stroke_width: f32) -> f32 {
    (stroke_width * 5.).max(16.)
}

/// The figures that draw `annotation`; empty for shapes drawn only as tiles.
pub fn outline(annotation: &Annotation) -> Vec<Figure> {
    let style = annotation.style();
    let color = style.color();
    let width = style.stroke_width();
    let stroke = |ops, is_round| Figure {
        ops,
        paint: Paint::Stroke { width, is_round },
        color,
    };
    let fill = |ops| Figure {
        ops,
        paint: Paint::Fill,
        color,
    };
    match annotation.shape() {
        Shape::Rectangle { from, to } => {
            let ops = rectangle(*from, *to);
            vec![if style.is_filled() {
                fill(ops)
            } else {
                stroke(ops, false)
            }]
        }
        Shape::Ellipse { from, to } => {
            let ops = ellipse(*from, *to);
            vec![if style.is_filled() {
                fill(ops)
            } else {
                stroke(ops, true)
            }]
        }
        Shape::Arrow { from, to } => vec![fill(arrow(*from, *to, width))],
        Shape::Line { from, to } => vec![stroke(
            vec![PathOp::MoveTo(*from), PathOp::LineTo(*to)],
            true,
        )],
        Shape::Pen { points } => vec![stroke(polyline(points), true)],
        Shape::Marker { points } => vec![Figure {
            ops: polyline(points),
            paint: Paint::Stroke {
                width: marker_width(width),
                is_round: true,
            },
            color: color.with_alpha(MARKER_ALPHA),
        }],
        Shape::Step { center, .. } => {
            let radius = step_radius(style.font_size());
            vec![fill(ellipse(
                ScenePoint::new(center.x - radius, center.y - radius),
                ScenePoint::new(center.x + radius, center.y + radius),
            ))]
        }
        Shape::Mosaic { .. }
        | Shape::Blur { .. }
        | Shape::Spotlight { .. }
        | Shape::Text { .. } => Vec::new(),
    }
}

fn rectangle(from: ScenePoint, to: ScenePoint) -> Vec<PathOp> {
    vec![
        PathOp::MoveTo(from),
        PathOp::LineTo(ScenePoint::new(to.x, from.y)),
        PathOp::LineTo(to),
        PathOp::LineTo(ScenePoint::new(from.x, to.y)),
        PathOp::Close,
    ]
}

/// An ellipse inscribed in the box from `from` to `to`, as four cubic arcs.
fn ellipse(from: ScenePoint, to: ScenePoint) -> Vec<PathOp> {
    // The control distance that makes a cubic Bézier closest to a quarter circle.
    const KAPPA: f32 = 0.552_284_8;
    let center = ScenePoint::new((from.x + to.x) / 2., (from.y + to.y) / 2.);
    let rx = (to.x - from.x).abs() / 2.;
    let ry = (to.y - from.y).abs() / 2.;
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let at = |dx: f32, dy: f32| ScenePoint::new(center.x + dx, center.y + dy);
    vec![
        PathOp::MoveTo(at(rx, 0.)),
        PathOp::CubicTo(at(rx, ky), at(kx, ry), at(0., ry)),
        PathOp::CubicTo(at(-kx, ry), at(-rx, ky), at(-rx, 0.)),
        PathOp::CubicTo(at(-rx, -ky), at(-kx, -ry), at(0., -ry)),
        PathOp::CubicTo(at(kx, -ry), at(rx, -ky), at(rx, 0.)),
        PathOp::Close,
    ]
}

/// A tapered arrow as one filled polygon: the shaft widens from a point at
/// the tail to the head, which scales with the stroke width but never
/// takes more than half of a short arrow.
pub fn arrow_polygon(from: ScenePoint, to: ScenePoint, width: f32) -> Vec<ScenePoint> {
    let length = from.distance(to);
    if length < f32::EPSILON {
        return Vec::new();
    }
    let (ux, uy) = ((to.x - from.x) / length, (to.y - from.y) / length);
    let (nx, ny) = (-uy, ux);
    let head_length = (width * 4. + 8.).min(length * 0.5);
    let head_half_width = head_length * 0.55;
    let shaft_half_width = (width * 0.75).min(head_half_width * 0.6);
    let neck = ScenePoint::new(to.x - ux * head_length * 0.8, to.y - uy * head_length * 0.8);
    let wing = ScenePoint::new(to.x - ux * head_length, to.y - uy * head_length);
    let offset = |point: ScenePoint, distance: f32| {
        ScenePoint::new(point.x + nx * distance, point.y + ny * distance)
    };
    vec![
        from,
        offset(neck, shaft_half_width),
        offset(wing, head_half_width),
        to,
        offset(wing, -head_half_width),
        offset(neck, -shaft_half_width),
    ]
}

fn arrow(from: ScenePoint, to: ScenePoint, width: f32) -> Vec<PathOp> {
    let polygon = arrow_polygon(from, to, width);
    let mut ops: Vec<PathOp> = polygon
        .iter()
        .enumerate()
        .map(|(ix, point)| {
            if ix == 0 {
                PathOp::MoveTo(*point)
            } else {
                PathOp::LineTo(*point)
            }
        })
        .collect();
    if !ops.is_empty() {
        ops.push(PathOp::Close);
    }
    ops
}

/// A freehand stroke. A single point becomes a zero-length segment, which
/// round caps draw as a dot.
fn polyline(points: &[ScenePoint]) -> Vec<PathOp> {
    let Some((first, rest)) = points.split_first() else {
        return Vec::new();
    };
    let mut ops = vec![PathOp::MoveTo(*first)];
    if rest.is_empty() {
        ops.push(PathOp::LineTo(ScenePoint::new(first.x + 0.01, first.y)));
    }
    ops.extend(rest.iter().map(|point| PathOp::LineTo(*point)));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{AnnotationId, Style};

    fn annotation(shape: Shape) -> Annotation {
        Annotation::new(
            AnnotationId(1),
            shape,
            Style::default().with_stroke_width(4.),
        )
    }

    #[test]
    fn test_arrow_polygon_tip_and_symmetry() {
        let from = ScenePoint::new(0., 0.);
        let to = ScenePoint::new(100., 0.);
        let polygon = arrow_polygon(from, to, 4.);
        assert_eq!(polygon.len(), 6);
        assert_eq!(polygon[0], from);
        assert_eq!(polygon[3], to, "the tip is the end point");
        assert!((polygon[2].y + polygon[4].y).abs() < 0.001, "wings mirror");
        assert!(polygon[2].x < to.x && polygon[2].x > 50.);
    }

    #[test]
    fn test_arrow_head_fits_short_arrows() {
        let polygon = arrow_polygon(ScenePoint::new(0., 0.), ScenePoint::new(10., 0.), 8.);
        assert!(polygon[2].x >= 5., "the head takes at most half the length");
    }

    #[test]
    fn test_ellipse_bounding_box() {
        let figures = outline(&annotation(Shape::Ellipse {
            from: ScenePoint::new(10., 20.),
            to: ScenePoint::new(50., 40.),
        }));
        let points: Vec<ScenePoint> = figures[0]
            .ops()
            .iter()
            .filter_map(|op| match op {
                PathOp::MoveTo(point) | PathOp::LineTo(point) => Some(*point),
                PathOp::CubicTo(_, _, point) => Some(*point),
                PathOp::Close => None,
            })
            .collect();
        let min_x = points.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        let max_y = points.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert_eq!((min_x, max_y), (10., 40.));
    }

    #[test]
    fn test_marker_is_translucent_and_wide() {
        let figures = outline(&annotation(Shape::Marker {
            points: [ScenePoint::new(0., 0.), ScenePoint::new(10., 0.)].into(),
        }));
        assert_eq!(figures[0].color().a, MARKER_ALPHA);
        assert!(matches!(figures[0].paint(), Paint::Stroke { width, .. } if width >= 12.));
    }

    #[test]
    fn test_tile_only_shapes_have_no_figures() {
        assert!(
            outline(&annotation(Shape::Text {
                origin: ScenePoint::default(),
                content: "hi".into()
            }))
            .is_empty()
        );
    }
}
