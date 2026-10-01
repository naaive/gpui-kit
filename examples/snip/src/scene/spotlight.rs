//! Spotlights: boxes that stay bright while the rest of the capture is
//! dimmed. The dimming belongs to the whole capture, not to one mark, so
//! export and the overlay both ask here for the area to dim.

use super::annotation::{Annotation, Color, ScenePoint, Shape};

/// What dims the capture outside the spotlights.
pub const SPOTLIGHT_SHADE: Color = Color::rgb(0, 0, 0).with_alpha(128);

/// The boxes of the spotlights among `annotations`, as least and greatest
/// corners on whole pixels: dimmed boxes then meet on pixel edges, where
/// neither the GPU nor export blends a seam between them.
pub fn spotlights<'a>(
    annotations: impl IntoIterator<Item = &'a Annotation>,
) -> Vec<(ScenePoint, ScenePoint)> {
    annotations
        .into_iter()
        .filter_map(|annotation| match annotation.shape() {
            Shape::Spotlight { from, to } => Some((
                ScenePoint::new(from.x.min(to.x).round(), from.y.min(to.y).round()),
                ScenePoint::new(from.x.max(to.x).round(), from.y.max(to.y).round()),
            )),
            _ => None,
        })
        .collect()
}

/// The parts of the box `min`–`max` outside every one of `holes`, as boxes
/// that don't overlap, so dimming each once dims the area evenly. Empty
/// when there are no holes: without a spotlight nothing is dimmed.
pub fn shaded(
    min: ScenePoint,
    max: ScenePoint,
    holes: &[(ScenePoint, ScenePoint)],
) -> Vec<(ScenePoint, ScenePoint)> {
    if holes.is_empty() || min.x >= max.x || min.y >= max.y {
        return Vec::new();
    }
    // Cut the area into a grid along every hole's edges; a cell is either
    // wholly inside a hole or wholly outside all of them.
    let clamp_x = |x: f32| x.clamp(min.x, max.x);
    let clamp_y = |y: f32| y.clamp(min.y, max.y);
    let mut xs = vec![min.x, max.x];
    let mut ys = vec![min.y, max.y];
    for (low, high) in holes {
        xs.extend([clamp_x(low.x), clamp_x(high.x)]);
        ys.extend([clamp_y(low.y), clamp_y(high.y)]);
    }
    for edges in [&mut xs, &mut ys] {
        edges.sort_by(f32::total_cmp);
        edges.dedup();
    }
    let mut boxes = Vec::new();
    for rows in ys.windows(2) {
        let (top, bottom) = (rows[0], rows[1]);
        // Cells next to each other in a row merge into one box.
        let mut run: Option<f32> = None;
        for columns in xs.windows(2) {
            let (left, right) = (columns[0], columns[1]);
            let center = ScenePoint::new((left + right) / 2., (top + bottom) / 2.);
            let is_lit = holes.iter().any(|(low, high)| {
                center.x > low.x && center.x < high.x && center.y > low.y && center.y < high.y
            });
            match (is_lit, run) {
                (false, None) => run = Some(left),
                (true, Some(start)) => {
                    boxes.push((ScenePoint::new(start, top), ScenePoint::new(left, bottom)));
                    run = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run {
            boxes.push((ScenePoint::new(start, top), ScenePoint::new(max.x, bottom)));
        }
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32) -> ScenePoint {
        ScenePoint::new(x, y)
    }

    fn area(boxes: &[(ScenePoint, ScenePoint)]) -> f32 {
        boxes
            .iter()
            .map(|(min, max)| (max.x - min.x) * (max.y - min.y))
            .sum()
    }

    #[test]
    fn test_nothing_is_dimmed_without_a_spotlight() {
        assert!(shaded(at(0., 0.), at(100., 100.), &[]).is_empty());
    }

    #[test]
    fn test_one_spotlight_leaves_a_frame() {
        let boxes = shaded(at(0., 0.), at(100., 100.), &[(at(20., 30.), at(60., 70.))]);
        assert_eq!(area(&boxes), 100. * 100. - 40. * 40.);
        assert_eq!(boxes.len(), 4, "above, left, right and below");
    }

    #[test]
    fn test_overlapping_spotlights_are_lit_once() {
        let holes = [(at(10., 10.), at(50., 50.)), (at(30., 30.), at(70., 70.))];
        let boxes = shaded(at(0., 0.), at(100., 100.), &holes);
        // Lit: two 40×40 boxes sharing a 20×20 corner.
        assert_eq!(area(&boxes), 100. * 100. - (1600. + 1600. - 400.));
    }

    #[test]
    fn test_spotlight_past_the_area_is_cut_to_it() {
        let boxes = shaded(
            at(0., 0.),
            at(100., 100.),
            &[(at(-50., -50.), at(50., 150.))],
        );
        assert_eq!(boxes, vec![(at(50., 0.), at(100., 100.))]);
    }
}
