//! Automatic selection: the window, or the part of one, under the pointer.

use super::{PhysPoint, PhysRect};

/// A top-level window as it was when the screen was frozen, with the
/// rectangles of its parts (child windows, controls) that can be selected
/// on their own.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowSnapshot {
    frame: PhysRect,
    parts: Vec<PhysRect>,
    /// The platform's handle for the window (an `HWND` on Windows), for
    /// looking up its controls after the capture; 0 where unknown.
    native_id: u64,
}

impl WindowSnapshot {
    pub fn new(frame: PhysRect) -> Self {
        Self {
            frame,
            parts: Vec::new(),
            native_id: 0,
        }
    }

    pub fn with_parts(mut self, parts: Vec<PhysRect>) -> Self {
        self.parts = parts;
        self
    }

    pub fn with_native_id(mut self, native_id: u64) -> Self {
        self.native_id = native_id;
        self
    }

    pub fn frame(&self) -> PhysRect {
        self.frame
    }

    pub fn native_id(&self) -> u64 {
        self.native_id
    }

    /// Adds parts found later, such as controls, skipping ones it has.
    pub fn add_parts(&mut self, parts: impl IntoIterator<Item = PhysRect>) {
        for part in parts {
            if part != self.frame && !self.parts.contains(&part) {
                self.parts.push(part);
            }
        }
    }
}

/// The region automatic selection offers at `point`: the smallest part
/// containing it in the frontmost window containing it, or that window.
///
/// `windows` is in z-order, frontmost first. A window hidden behind
/// another at `point` is never offered, even if it is smaller.
pub fn region_at(windows: &[WindowSnapshot], point: PhysPoint) -> Option<PhysRect> {
    let window = windows.iter().find(|window| window.frame.contains(point))?;
    Some(
        window
            .parts
            .iter()
            .filter(|part| part.contains(point))
            .min_by_key(|part| part.area())
            .copied()
            .unwrap_or(window.frame),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_region_at_frontmost_window_wins() {
        let front = WindowSnapshot::new(PhysRect::new(0, 0, 400, 300));
        let back = WindowSnapshot::new(PhysRect::new(100, 100, 50, 50));
        let windows = [front.clone(), back];
        assert_eq!(
            region_at(&windows, PhysPoint::new(120, 120)),
            Some(front.frame),
            "the smaller window behind is hidden at this point"
        );
    }

    #[test]
    fn test_region_at_prefers_smallest_part() {
        let window = WindowSnapshot::new(PhysRect::new(0, 0, 400, 300)).with_parts(vec![
            PhysRect::new(0, 0, 400, 40),
            PhysRect::new(10, 10, 80, 20),
        ]);
        assert_eq!(
            region_at(std::slice::from_ref(&window), PhysPoint::new(20, 15)),
            Some(PhysRect::new(10, 10, 80, 20))
        );
        assert_eq!(
            region_at(std::slice::from_ref(&window), PhysPoint::new(200, 200)),
            Some(window.frame)
        );
        assert_eq!(region_at(&[window], PhysPoint::new(500, 500)), None);
    }
}
