//! Which annotation is under the pointer, and the box that marks it as
//! selected.
//!
//! Outlined shapes are hit on their stroke, not their inside, so a box
//! drawn around something doesn't stop the user from drawing inside it;
//! filled shapes, text and step markers are hit anywhere on them.

use super::{
    annotation::{Annotation, ScenePoint, Shape},
    outline::{arrow_polygon, marker_width, mosaic_width, step_radius},
};

/// Whether `point` is on `annotation`, within `tolerance` pixels.
pub fn hits(annotation: &Annotation, point: ScenePoint, tolerance: f32) -> bool {
    let style = annotation.style();
    let reach = style.stroke_width() / 2. + tolerance;
    match annotation.shape() {
        Shape::Rectangle { from, to } => {
            let (min, max) = corners(*from, *to);
            if style.is_filled() {
                return inside_by(point, min, max, reach);
            }
            near_box_edge(point, min, max, reach)
        }
        Shape::Ellipse { from, to } => {
            let (min, max) = corners(*from, *to);
            let center = ScenePoint::new((min.x + max.x) / 2., (min.y + max.y) / 2.);
            let (rx, ry) = ((max.x - min.x) / 2., (max.y - min.y) / 2.);
            if rx < f32::EPSILON || ry < f32::EPSILON {
                return distance_to_segment(point, min, max) <= reach;
            }
            let (dx, dy) = ((point.x - center.x) / rx, (point.y - center.y) / ry);
            let normalized = dx.hypot(dy);
            if style.is_filled() {
                return normalized <= 1. + reach / rx.min(ry);
            }
            (normalized - 1.).abs() * rx.min(ry) <= reach
        }
        Shape::Line { from, to } => distance_to_segment(point, *from, *to) <= reach,
        Shape::Arrow { from, to } => {
            let polygon = arrow_polygon(*from, *to, style.stroke_width());
            inside_polygon(point, &polygon) || distance_to_segment(point, *from, *to) <= reach
        }
        Shape::Pen { points } => near_polyline(point, points, reach),
        Shape::Marker { points } => near_polyline(
            point,
            points,
            marker_width(style.stroke_width()) / 2. + tolerance,
        ),
        Shape::Mosaic { points } | Shape::Blur { points } => near_polyline(
            point,
            points,
            mosaic_width(style.stroke_width()) / 2. + tolerance,
        ),
        // On the edge between bright and dim, so drawing inside still works.
        Shape::Spotlight { from, to } => {
            let (min, max) = corners(*from, *to);
            near_box_edge(point, min, max, tolerance.max(2.))
        }
        Shape::Text { .. } => {
            let (min, max) = bounds(annotation);
            inside_by(point, min, max, tolerance)
        }
        Shape::Step { center, .. } => {
            center.distance(point) <= step_radius(style.font_size()) + tolerance
        }
    }
}

/// Whether `point` is within `reach` of the edge of the box `min`–`max`.
fn near_box_edge(point: ScenePoint, min: ScenePoint, max: ScenePoint, reach: f32) -> bool {
    [
        distance_to_segment(point, min, ScenePoint::new(max.x, min.y)),
        distance_to_segment(point, ScenePoint::new(max.x, min.y), max),
        distance_to_segment(point, max, ScenePoint::new(min.x, max.y)),
        distance_to_segment(point, ScenePoint::new(min.x, max.y), min),
    ]
    .into_iter()
    .fold(f32::MAX, f32::min)
        <= reach
}

/// The frontmost annotation under `point`.
pub fn topmost_at(
    annotations: &[std::sync::Arc<Annotation>],
    point: ScenePoint,
    tolerance: f32,
) -> Option<&std::sync::Arc<Annotation>> {
    annotations
        .iter()
        .rev()
        .find(|annotation| hits(annotation, point, tolerance))
}

/// The box around everything `annotation` draws, as its least and greatest
/// corners. Text is measured roughly, from its font size and characters.
pub fn bounds(annotation: &Annotation) -> (ScenePoint, ScenePoint) {
    let style = annotation.style();
    let pad = |(min, max): (ScenePoint, ScenePoint), by: f32| {
        (
            ScenePoint::new(min.x - by, min.y - by),
            ScenePoint::new(max.x + by, max.y + by),
        )
    };
    let of_points = |points: &[ScenePoint]| {
        points.iter().fold(
            (
                ScenePoint::new(f32::MAX, f32::MAX),
                ScenePoint::new(f32::MIN, f32::MIN),
            ),
            |(min, max), point| {
                (
                    ScenePoint::new(min.x.min(point.x), min.y.min(point.y)),
                    ScenePoint::new(max.x.max(point.x), max.y.max(point.y)),
                )
            },
        )
    };
    let half = style.stroke_width() / 2.;
    match annotation.shape() {
        Shape::Rectangle { from, to } | Shape::Ellipse { from, to } => {
            pad(corners(*from, *to), half)
        }
        Shape::Line { from, to } => pad(corners(*from, *to), half),
        Shape::Arrow { from, to } => of_points(&arrow_polygon(*from, *to, style.stroke_width())),
        Shape::Pen { points } => pad(of_points(points), half),
        Shape::Marker { points } => pad(of_points(points), marker_width(style.stroke_width()) / 2.),
        Shape::Mosaic { points } | Shape::Blur { points } => {
            pad(of_points(points), mosaic_width(style.stroke_width()) / 2.)
        }
        Shape::Spotlight { from, to } => corners(*from, *to),
        Shape::Text { origin, content } => {
            let size = style.font_size();
            let lines = content.lines().count().max(1) as f32;
            let longest = content
                .lines()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0);
            (
                *origin,
                ScenePoint::new(
                    origin.x + longest as f32 * size * 0.6,
                    origin.y + lines * size * 1.3,
                ),
            )
        }
        Shape::Step { center, .. } => {
            let radius = step_radius(style.font_size());
            (
                ScenePoint::new(center.x - radius, center.y - radius),
                ScenePoint::new(center.x + radius, center.y + radius),
            )
        }
    }
}

fn corners(from: ScenePoint, to: ScenePoint) -> (ScenePoint, ScenePoint) {
    (
        ScenePoint::new(from.x.min(to.x), from.y.min(to.y)),
        ScenePoint::new(from.x.max(to.x), from.y.max(to.y)),
    )
}

fn inside_by(point: ScenePoint, min: ScenePoint, max: ScenePoint, by: f32) -> bool {
    point.x >= min.x - by && point.x <= max.x + by && point.y >= min.y - by && point.y <= max.y + by
}

fn distance_to_segment(point: ScenePoint, from: ScenePoint, to: ScenePoint) -> f32 {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length_squared = dx * dx + dy * dy;
    if length_squared < f32::EPSILON {
        return point.distance(from);
    }
    let t = (((point.x - from.x) * dx + (point.y - from.y) * dy) / length_squared).clamp(0., 1.);
    point.distance(ScenePoint::new(from.x + t * dx, from.y + t * dy))
}

fn near_polyline(point: ScenePoint, points: &[ScenePoint], reach: f32) -> bool {
    match points {
        [] => false,
        [only] => only.distance(point) <= reach,
        _ => points
            .windows(2)
            .any(|pair| distance_to_segment(point, pair[0], pair[1]) <= reach),
    }
}

/// Even-odd point-in-polygon.
fn inside_polygon(point: ScenePoint, polygon: &[ScenePoint]) -> bool {
    let mut inside = false;
    let mut previous = match polygon.last() {
        Some(last) => *last,
        None => return false,
    };
    for current in polygon {
        if (current.y > point.y) != (previous.y > point.y)
            && point.x
                < (previous.x - current.x) * (point.y - current.y) / (previous.y - current.y)
                    + current.x
        {
            inside = !inside;
        }
        previous = *current;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{AnnotationId, Style};

    fn annotation(shape: Shape, style: Style) -> Annotation {
        Annotation::new(AnnotationId(1), shape, style)
    }

    fn at(x: f32, y: f32) -> ScenePoint {
        ScenePoint::new(x, y)
    }

    #[test]
    fn test_outlined_rectangle_is_hit_on_its_edge() {
        let rectangle = annotation(
            Shape::Rectangle {
                from: at(0., 0.),
                to: at(100., 100.),
            },
            Style::default().with_stroke_width(4.),
        );
        assert!(hits(&rectangle, at(1., 50.), 3.));
        assert!(!hits(&rectangle, at(50., 50.), 3.), "the inside is free");
        let filled = annotation(
            rectangle.shape().clone(),
            Style::default().with_filled(true),
        );
        assert!(hits(&filled, at(50., 50.), 3.));
    }

    #[test]
    fn test_line_and_ellipse_reach_grows_with_width() {
        let line = annotation(
            Shape::Line {
                from: at(0., 0.),
                to: at(100., 0.),
            },
            Style::default().with_stroke_width(8.),
        );
        assert!(hits(&line, at(50., 6.), 3.));
        assert!(!hits(&line, at(50., 9.), 3.));

        let ellipse = annotation(
            Shape::Ellipse {
                from: at(0., 0.),
                to: at(100., 50.),
            },
            Style::default().with_stroke_width(2.),
        );
        assert!(hits(&ellipse, at(100., 25.), 2.));
        assert!(!hits(&ellipse, at(50., 25.), 2.));
    }

    #[test]
    fn test_topmost_wins() {
        let below = std::sync::Arc::new(Annotation::new(
            AnnotationId(1),
            Shape::Step {
                center: at(10., 10.),
                number: 1,
            },
            Style::default(),
        ));
        let above = std::sync::Arc::new(Annotation::new(
            AnnotationId(2),
            Shape::Step {
                center: at(12., 12.),
                number: 2,
            },
            Style::default(),
        ));
        let annotations = [below, above];
        assert_eq!(
            topmost_at(&annotations, at(11., 11.), 2.).map(|annotation| annotation.id()),
            Some(AnnotationId(2))
        );
        assert!(topmost_at(&annotations, at(200., 200.), 2.).is_none());
    }

    #[test]
    fn test_arrow_head_and_text_box() {
        let arrow = annotation(
            Shape::Arrow {
                from: at(0., 0.),
                to: at(100., 0.),
            },
            Style::default().with_stroke_width(4.),
        );
        assert!(hits(&arrow, at(90., 3.), 0.), "inside the head");
        let text = annotation(
            Shape::Text {
                origin: at(10., 10.),
                content: "Hello\nworld!".into(),
            },
            Style::default().with_font_size(20.),
        );
        let (min, max) = bounds(&text);
        assert_eq!(min, at(10., 10.));
        assert!(max.x > 70. && max.y > 60.);
        assert!(hits(&text, at(20., 40.), 0.));
    }
}
