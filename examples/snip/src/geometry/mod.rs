//! Pixel geometry of a capture.
//!
//! Every coordinate here is a physical pixel of the virtual desktop: the
//! space the operating system places monitors in, before any scaling. A
//! frozen frame is stored in those pixels, so a selection made in them crops
//! exactly what was on screen. Logical pixels, which GPUI lays out in, only
//! appear at the window boundary, through [`DisplayArea`].

mod handles;
mod snap;

pub use handles::{Grip, Handle, grip_at};
pub use snap::{WindowSnapshot, region_at};

/// A physical pixel position on the virtual desktop.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct PhysPoint {
    pub x: i32,
    pub y: i32,
}

impl PhysPoint {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// A rectangle of whole physical pixels: `x..x + width` by `y..y + height`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl PhysRect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The rectangle a drag from `anchor` to `current` spans, whichever way
    /// it went.
    pub fn from_corners(anchor: PhysPoint, current: PhysPoint) -> Self {
        let x = anchor.x.min(current.x);
        let y = anchor.y.min(current.y);
        Self::new(
            x,
            y,
            anchor.x.max(current.x) - x,
            anchor.y.max(current.y) - y,
        )
    }

    /// The rectangle between two edges on each axis, in either order.
    pub fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self::from_corners(PhysPoint::new(left, top), PhysPoint::new(right, bottom))
    }

    pub const fn origin(&self) -> PhysPoint {
        PhysPoint::new(self.x, self.y)
    }

    pub const fn right(&self) -> i32 {
        self.x + self.width
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.height
    }

    pub const fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub const fn area(&self) -> i64 {
        self.width as i64 * self.height as i64
    }

    pub const fn contains(&self, point: PhysPoint) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }

    pub fn intersect(&self, other: &PhysRect) -> Option<PhysRect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > x && bottom > y).then(|| PhysRect::new(x, y, right - x, bottom - y))
    }

    pub const fn translate(&self, dx: i32, dy: i32) -> PhysRect {
        PhysRect::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// The nearest point inside the rectangle, for a pointer that left it.
    pub fn clamp_point(&self, point: PhysPoint) -> PhysPoint {
        PhysPoint::new(
            point.x.clamp(self.x, self.right()),
            point.y.clamp(self.y, self.bottom()),
        )
    }

    /// Moves the rectangle back inside `bounds` without resizing it, unless
    /// it is larger, in which case it is cut to `bounds`.
    pub fn move_within(&self, bounds: &PhysRect) -> PhysRect {
        let width = self.width.min(bounds.width);
        let height = self.height.min(bounds.height);
        PhysRect::new(
            self.x.clamp(bounds.x, bounds.right() - width),
            self.y.clamp(bounds.y, bounds.bottom() - height),
            width,
            height,
        )
    }
}

/// One display's place on the virtual desktop and its scale, which turns
/// GPUI's logical window coordinates into physical desktop pixels and back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayArea {
    bounds: PhysRect,
    scale: f32,
}

impl DisplayArea {
    pub fn new(bounds: PhysRect, scale: f32) -> Self {
        Self {
            bounds,
            scale: if scale > 0. { scale } else { 1. },
        }
    }

    pub fn bounds(&self) -> PhysRect {
        self.bounds
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The physical desktop position of a logical position in a window that
    /// covers this display.
    pub fn to_physical(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.bounds.x as f32 + x * self.scale,
            self.bounds.y as f32 + y * self.scale,
        )
    }

    /// The logical position, in a window covering this display, of a
    /// physical desktop position.
    pub fn to_logical(&self, x: f32, y: f32) -> (f32, f32) {
        (
            (x - self.bounds.x as f32) / self.scale,
            (y - self.bounds.y as f32) / self.scale,
        )
    }

    /// A physical length as logical pixels.
    pub fn to_logical_length(&self, length: f32) -> f32 {
        length / self.scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_corners_normalizes_every_direction() {
        let expected = PhysRect::new(10, 20, 30, 40);
        for (anchor, current) in [
            ((10, 20), (40, 60)),
            ((40, 60), (10, 20)),
            ((40, 20), (10, 60)),
            ((10, 60), (40, 20)),
        ] {
            assert_eq!(
                PhysRect::from_corners(
                    PhysPoint::new(anchor.0, anchor.1),
                    PhysPoint::new(current.0, current.1)
                ),
                expected
            );
        }
    }

    #[test]
    fn test_contains_is_half_open() {
        let rect = PhysRect::new(0, 0, 10, 10);
        assert!(rect.contains(PhysPoint::new(0, 0)));
        assert!(rect.contains(PhysPoint::new(9, 9)));
        assert!(!rect.contains(PhysPoint::new(10, 9)));
        assert!(!rect.contains(PhysPoint::new(-1, 0)));
    }

    #[test]
    fn test_intersect() {
        let a = PhysRect::new(0, 0, 10, 10);
        assert_eq!(
            a.intersect(&PhysRect::new(5, 5, 10, 10)),
            Some(PhysRect::new(5, 5, 5, 5))
        );
        assert_eq!(a.intersect(&PhysRect::new(10, 0, 5, 5)), None);
    }

    #[test]
    fn test_move_within() {
        let bounds = PhysRect::new(-1920, 0, 1920, 1080);
        assert_eq!(
            PhysRect::new(-10, 1070, 100, 50).move_within(&bounds),
            PhysRect::new(-100, 1030, 100, 50)
        );
        assert_eq!(
            PhysRect::new(-3000, -5, 4000, 50).move_within(&bounds),
            PhysRect::new(-1920, 0, 1920, 50),
            "a rectangle larger than the bounds is cut to them"
        );
    }

    #[test]
    fn test_display_area_round_trip_at_common_scales() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let area = DisplayArea::new(PhysRect::new(-2560, -200, 2560, 1440), scale);
            let (x, y) = area.to_physical(100., 40.);
            assert_eq!((x, y), (-2560. + 100. * scale, -200. + 40. * scale));
            assert_eq!(area.to_logical(x, y), (100., 40.));
        }
    }
}
