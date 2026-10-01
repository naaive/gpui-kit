//! The grips of a selection: eight resize handles and the body that moves it.

use super::{PhysPoint, PhysRect};

/// A resize handle on a selection's edge or corner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
    ];

    /// Which edges the handle moves: (left, top, right, bottom).
    const fn edges(self) -> (bool, bool, bool, bool) {
        match self {
            Self::TopLeft => (true, true, false, false),
            Self::Top => (false, true, false, false),
            Self::TopRight => (false, true, true, false),
            Self::Right => (false, false, true, false),
            Self::BottomRight => (false, false, true, true),
            Self::Bottom => (false, false, false, true),
            Self::BottomLeft => (true, false, false, true),
            Self::Left => (true, false, false, false),
        }
    }

    /// Where the handle sits on `rect`.
    pub fn position(self, rect: &PhysRect) -> PhysPoint {
        let center_x = rect.x + rect.width / 2;
        let center_y = rect.y + rect.height / 2;
        let (left, top, right, bottom) = self.edges();
        PhysPoint::new(
            if left {
                rect.x
            } else if right {
                rect.right()
            } else {
                center_x
            },
            if top {
                rect.y
            } else if bottom {
                rect.bottom()
            } else {
                center_y
            },
        )
    }

    /// Whether dragging this handle resizes along a diagonal, a column or a row;
    /// picks the resize cursor.
    pub fn is_diagonal_down(self) -> bool {
        matches!(self, Self::TopLeft | Self::BottomRight)
    }

    pub fn is_diagonal_up(self) -> bool {
        matches!(self, Self::TopRight | Self::BottomLeft)
    }

    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }
}

/// What the pointer holds when it presses on a selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Grip {
    Handle(Handle),
    /// Inside the selection: the drag moves it.
    Body,
}

impl Grip {
    /// The rectangle after dragging this grip of `start` by `(dx, dy)`.
    ///
    /// A handle dragged past the opposite edge flips the rectangle rather
    /// than collapsing it, as a selection does in every image editor.
    pub fn drag(self, start: &PhysRect, dx: i32, dy: i32) -> PhysRect {
        match self {
            Self::Body => start.translate(dx, dy),
            Self::Handle(handle) => {
                let (left, top, right, bottom) = handle.edges();
                PhysRect::from_edges(
                    start.x + if left { dx } else { 0 },
                    start.y + if top { dy } else { 0 },
                    start.right() + if right { dx } else { 0 },
                    start.bottom() + if bottom { dy } else { 0 },
                )
            }
        }
    }
}

/// The grip of `rect` under `point`, if any. Handles win over the body
/// within `tolerance` physical pixels, so a small selection can still be
/// resized from its corners.
pub fn grip_at(rect: &PhysRect, point: PhysPoint, tolerance: i32) -> Option<Grip> {
    let near = |target: PhysPoint| {
        (point.x - target.x).abs() <= tolerance && (point.y - target.y).abs() <= tolerance
    };
    if let Some(handle) = Handle::ALL
        .into_iter()
        .find(|handle| near(handle.position(rect)))
    {
        return Some(Grip::Handle(handle));
    }
    let within_x = point.x >= rect.x - tolerance && point.x <= rect.right() + tolerance;
    let within_y = point.y >= rect.y - tolerance && point.y <= rect.bottom() + tolerance;
    if within_x && (point.y - rect.y).abs() <= tolerance {
        return Some(Grip::Handle(Handle::Top));
    }
    if within_x && (point.y - rect.bottom()).abs() <= tolerance {
        return Some(Grip::Handle(Handle::Bottom));
    }
    if within_y && (point.x - rect.x).abs() <= tolerance {
        return Some(Grip::Handle(Handle::Left));
    }
    if within_y && (point.x - rect.right()).abs() <= tolerance {
        return Some(Grip::Handle(Handle::Right));
    }
    rect.contains(point).then_some(Grip::Body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: PhysRect = PhysRect::new(100, 100, 200, 100);

    #[test]
    fn test_grip_at_prefers_handles() {
        assert_eq!(
            grip_at(&RECT, PhysPoint::new(102, 98), 4),
            Some(Grip::Handle(Handle::TopLeft))
        );
        assert_eq!(
            grip_at(&RECT, PhysPoint::new(200, 201), 4),
            Some(Grip::Handle(Handle::Bottom))
        );
        assert_eq!(
            grip_at(&RECT, PhysPoint::new(150, 102), 4),
            Some(Grip::Handle(Handle::Top)),
            "anywhere along an edge resizes"
        );
        assert_eq!(
            grip_at(&RECT, PhysPoint::new(200, 150), 4),
            Some(Grip::Body)
        );
        assert_eq!(grip_at(&RECT, PhysPoint::new(50, 50), 4), None);
    }

    #[test]
    fn test_drag_handles_and_flip() {
        assert_eq!(
            Grip::Handle(Handle::BottomRight).drag(&RECT, 10, 20),
            PhysRect::new(100, 100, 210, 120)
        );
        assert_eq!(
            Grip::Handle(Handle::Left).drag(&RECT, 250, 30),
            PhysRect::new(300, 100, 50, 100),
            "the left edge dragged past the right one flips, and the row is unchanged"
        );
        assert_eq!(
            Grip::Body.drag(&RECT, -5, 5),
            PhysRect::new(95, 105, 200, 100)
        );
    }
}
