//! Where each table goes on the diagram.
//!
//! Tables are laid out in layers by their foreign keys: a table that others
//! refer to stands to the left of the tables that refer to it, so references
//! read left to right. A layer taller than the window wraps into another
//! column. Units are rems, so the diagram scales with the interface.

use std::collections::HashMap;

/// A table's size on the diagram and what it refers to.
pub struct Shape {
    pub height: f32,
    /// Indexes of the tables its foreign keys refer to.
    pub references: Vec<usize>,
}

pub const BOX_WIDTH: f32 = 15.0;
pub const HORIZONTAL_GAP: f32 = 5.0;
pub const VERTICAL_GAP: f32 = 2.0;
/// How tall a layer grows before it wraps into another column.
pub const MAX_COLUMN_HEIGHT: f32 = 60.0;

/// The top-left corner of every shape, in the order given.
pub fn layout(shapes: &[Shape]) -> Vec<(f32, f32)> {
    let layers = layers(shapes);
    let mut by_layer: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ix, layer) in layers.iter().enumerate() {
        by_layer.entry(*layer).or_default().push(ix);
    }
    let mut positions = vec![(0.0, 0.0); shapes.len()];
    let mut x = 0.0;
    let mut layer_numbers: Vec<usize> = by_layer.keys().copied().collect();
    layer_numbers.sort_unstable();
    for layer in layer_numbers {
        let mut y = 0.0;
        for &ix in &by_layer[&layer] {
            if y > 0.0 && y + shapes[ix].height > MAX_COLUMN_HEIGHT {
                x += BOX_WIDTH + HORIZONTAL_GAP;
                y = 0.0;
            }
            positions[ix] = (x, y);
            y += shapes[ix].height + VERTICAL_GAP;
        }
        x += BOX_WIDTH + HORIZONTAL_GAP;
    }
    positions
}

/// Each shape's layer: 0 for a table that refers to nothing, otherwise one
/// more than the deepest table it refers to. A reference cycle is cut where
/// the walk meets it again.
fn layers(shapes: &[Shape]) -> Vec<usize> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Unvisited,
        Visiting,
        Done(usize),
    }
    fn visit(ix: usize, shapes: &[Shape], marks: &mut [Mark]) -> usize {
        match marks[ix] {
            Mark::Done(layer) => return layer,
            Mark::Visiting => return 0,
            Mark::Unvisited => {}
        }
        marks[ix] = Mark::Visiting;
        let layer = shapes[ix]
            .references
            .iter()
            .filter(|&&referenced| referenced != ix && referenced < shapes.len())
            .map(|&referenced| visit(referenced, shapes, marks) + 1)
            .max()
            .unwrap_or(0);
        marks[ix] = Mark::Done(layer);
        layer
    }
    let mut marks = vec![Mark::Unvisited; shapes.len()];
    (0..shapes.len())
        .map(|ix| visit(ix, shapes, &mut marks))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(references: &[usize]) -> Shape {
        Shape {
            height: 10.0,
            references: references.to_vec(),
        }
    }

    #[test]
    fn referenced_tables_stand_to_the_left() {
        // items → orders → customers
        let shapes = [shape(&[1]), shape(&[2]), shape(&[])];
        let positions = layout(&shapes);
        assert!(positions[2].0 < positions[1].0);
        assert!(positions[1].0 < positions[0].0);
    }

    #[test]
    fn a_cycle_does_not_loop() {
        let shapes = [shape(&[1]), shape(&[0]), shape(&[0])];
        let positions = layout(&shapes);
        assert_eq!(positions.len(), 3);
    }

    #[test]
    fn a_tall_layer_wraps() {
        let shapes: Vec<Shape> = (0..10).map(|_| shape(&[])).collect();
        let positions = layout(&shapes);
        let columns: std::collections::BTreeSet<i64> =
            positions.iter().map(|(x, _)| *x as i64).collect();
        assert!(columns.len() > 1);
        assert!(positions.iter().all(|(_, y)| *y <= MAX_COLUMN_HEIGHT));
    }
}
