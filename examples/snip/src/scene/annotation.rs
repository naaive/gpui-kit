//! What the user draws on a capture.
//!
//! Annotations live in the same physical desktop pixels as the frozen
//! frames, not relative to the selection, so resizing the selection after
//! drawing leaves every mark where it was; export crops them with the image.

use std::sync::Arc;

/// A point on the virtual desktop in physical pixels, with sub-pixel
/// precision for strokes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScenePoint {
    pub x: f32,
    pub y: f32,
}

impl ScenePoint {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: ScenePoint) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

/// An 8-bit straight-alpha color. Annotation colors are the user's content,
/// not interface chrome, so they are plain values rather than theme tokens.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// `#RRGGBB`, the form the color picker readout and copy use.
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Black or white, whichever reads better on this color.
    pub fn contrasting(self) -> Color {
        let luminance = 0.299 * self.r as f32 + 0.587 * self.g as f32 + 0.114 * self.b as f32;
        if luminance > 150. {
            Color::rgb(0, 0, 0)
        } else {
            Color::rgb(255, 255, 255)
        }
    }
}

/// The colors offered for annotations, in order.
pub const PALETTE: [Color; 8] = [
    Color::rgb(0xF0, 0x3E, 0x3E),
    Color::rgb(0xF5, 0x9F, 0x00),
    Color::rgb(0xFA, 0xD7, 0x14),
    Color::rgb(0x37, 0xB2, 0x4D),
    Color::rgb(0x1C, 0x7E, 0xD6),
    Color::rgb(0x70, 0x48, 0xE8),
    Color::rgb(0x00, 0x00, 0x00),
    Color::rgb(0xFF, 0xFF, 0xFF),
];

/// The drawing tools, in toolbar order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Tool {
    Rectangle,
    Ellipse,
    Arrow,
    Line,
    Pen,
    Marker,
    Mosaic,
    Blur,
    Spotlight,
    Text,
    Step,
}

impl Tool {
    pub const ALL: [Self; 11] = [
        Self::Rectangle,
        Self::Ellipse,
        Self::Arrow,
        Self::Line,
        Self::Pen,
        Self::Marker,
        Self::Mosaic,
        Self::Blur,
        Self::Spotlight,
        Self::Text,
        Self::Step,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Arrow => "Arrow",
            Self::Line => "Line",
            Self::Pen => "Pen",
            Self::Marker => "Marker",
            Self::Mosaic => "Mosaic",
            Self::Blur => "Blur",
            Self::Spotlight => "Spotlight",
            Self::Text => "Text",
            Self::Step => "Step number",
        }
    }

    /// Whether the tool draws a filled shape when fill is on.
    pub fn can_fill(self) -> bool {
        matches!(self, Self::Rectangle | Self::Ellipse)
    }

    /// Whether the stroke width setting applies; text and step markers size
    /// by font size instead.
    pub fn has_stroke(self) -> bool {
        !matches!(self, Self::Text | Self::Step | Self::Spotlight)
    }

    /// Whether the color setting applies: mosaic and blur show the capture
    /// itself, and a spotlight dims around a box.
    pub fn has_color(self) -> bool {
        !matches!(self, Self::Mosaic | Self::Blur | Self::Spotlight)
    }

    /// Whether the tool has any style to choose.
    pub fn has_style(self) -> bool {
        self != Self::Spotlight
    }
}

/// Stroke widths offered, in logical pixels.
pub const STROKE_WIDTHS: [f32; 3] = [2., 4., 8.];

/// Font sizes offered for text and step markers, in logical pixels.
pub const FONT_SIZES: [f32; 3] = [14., 18., 24.];

/// How the next annotation will look, in logical pixels: the overlay turns
/// sizes into physical pixels with the scale of the display drawn on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    color: Color,
    stroke_width: f32,
    font_size: f32,
    is_filled: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color: PALETTE[0],
            stroke_width: STROKE_WIDTHS[1],
            font_size: FONT_SIZES[1],
            is_filled: false,
        }
    }
}

impl Style {
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn with_stroke_width(mut self, width: f32) -> Self {
        self.stroke_width = width.max(1.);
        self
    }

    pub fn with_font_size(mut self, size: f32) -> Self {
        self.font_size = size.max(6.);
        self
    }

    pub fn with_filled(mut self, is_filled: bool) -> Self {
        self.is_filled = is_filled;
        self
    }

    pub fn color(&self) -> Color {
        self.color
    }

    pub fn stroke_width(&self) -> f32 {
        self.stroke_width
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    pub fn is_filled(&self) -> bool {
        self.is_filled
    }

    /// The next stroke width up or down the offered steps.
    pub fn step_stroke_width(self, wider: bool) -> Self {
        let ix = STROKE_WIDTHS
            .iter()
            .position(|width| *width >= self.stroke_width)
            .unwrap_or(STROKE_WIDTHS.len() - 1);
        let ix = if wider {
            (ix + 1).min(STROKE_WIDTHS.len() - 1)
        } else {
            ix.saturating_sub(1)
        };
        self.with_stroke_width(STROKE_WIDTHS[ix])
    }

    /// This style at a display's scale, for an annotation drawn there.
    pub fn to_physical(self, scale: f32) -> Style {
        Self {
            stroke_width: self.stroke_width * scale,
            font_size: self.font_size * scale,
            ..self
        }
    }
}

/// The geometry of an annotation, in physical desktop pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rectangle {
        from: ScenePoint,
        to: ScenePoint,
    },
    Ellipse {
        from: ScenePoint,
        to: ScenePoint,
    },
    Arrow {
        from: ScenePoint,
        to: ScenePoint,
    },
    Line {
        from: ScenePoint,
        to: ScenePoint,
    },
    Pen {
        points: Arc<[ScenePoint]>,
    },
    Marker {
        points: Arc<[ScenePoint]>,
    },
    /// Pixelates the frozen frame along a brush stroke.
    Mosaic {
        points: Arc<[ScenePoint]>,
    },
    /// Blurs the frozen frame along a brush stroke.
    Blur {
        points: Arc<[ScenePoint]>,
    },
    /// Keeps a box bright and dims the rest of the capture around it; with
    /// several, everything outside all of them is dimmed once.
    Spotlight {
        from: ScenePoint,
        to: ScenePoint,
    },
    /// `origin` is the top-left of the first line.
    Text {
        origin: ScenePoint,
        content: Arc<str>,
    },
    /// A numbered badge centred on `center`.
    Step {
        center: ScenePoint,
        number: u32,
    },
}

impl Shape {
    /// The shape `tool` starts when the pointer presses at `point`.
    /// Text and step markers are not dragged out, so they have none.
    pub fn start(tool: Tool, point: ScenePoint) -> Option<Shape> {
        let points = || Arc::from([point]);
        Some(match tool {
            Tool::Rectangle => Shape::Rectangle {
                from: point,
                to: point,
            },
            Tool::Ellipse => Shape::Ellipse {
                from: point,
                to: point,
            },
            Tool::Arrow => Shape::Arrow {
                from: point,
                to: point,
            },
            Tool::Line => Shape::Line {
                from: point,
                to: point,
            },
            Tool::Pen => Shape::Pen { points: points() },
            Tool::Marker => Shape::Marker { points: points() },
            Tool::Mosaic => Shape::Mosaic { points: points() },
            Tool::Blur => Shape::Blur { points: points() },
            Tool::Spotlight => Shape::Spotlight {
                from: point,
                to: point,
            },
            Tool::Text | Tool::Step => return None,
        })
    }

    /// The shape after the pointer moved to `point` while drawing.
    ///
    /// With `is_constrained` (Shift held), rectangles and ellipses become
    /// squares and circles and lines snap to 45° steps.
    pub fn extend(&self, point: ScenePoint, is_constrained: bool) -> Shape {
        let constrain_box = |from: ScenePoint| {
            if !is_constrained {
                return point;
            }
            let side = (point.x - from.x).abs().max((point.y - from.y).abs());
            ScenePoint::new(
                from.x + side.copysign(point.x - from.x),
                from.y + side.copysign(point.y - from.y),
            )
        };
        let constrain_angle = |from: ScenePoint| {
            if !is_constrained {
                return point;
            }
            let (dx, dy) = (point.x - from.x, point.y - from.y);
            let step = std::f32::consts::FRAC_PI_4;
            let angle = (dy.atan2(dx) / step).round() * step;
            let length = dx.hypot(dy);
            ScenePoint::new(from.x + length * angle.cos(), from.y + length * angle.sin())
        };
        let append = |points: &Arc<[ScenePoint]>| -> Arc<[ScenePoint]> {
            // Points closer than a pixel add nothing but tessellation work.
            if points.last().is_some_and(|last| last.distance(point) < 1.) {
                return points.clone();
            }
            points.iter().copied().chain([point]).collect()
        };
        match self {
            Shape::Rectangle { from, .. } => Shape::Rectangle {
                from: *from,
                to: constrain_box(*from),
            },
            Shape::Ellipse { from, .. } => Shape::Ellipse {
                from: *from,
                to: constrain_box(*from),
            },
            Shape::Arrow { from, .. } => Shape::Arrow {
                from: *from,
                to: constrain_angle(*from),
            },
            Shape::Line { from, .. } => Shape::Line {
                from: *from,
                to: constrain_angle(*from),
            },
            Shape::Pen { points } => Shape::Pen {
                points: append(points),
            },
            Shape::Marker { points } => Shape::Marker {
                points: append(points),
            },
            Shape::Mosaic { points } => Shape::Mosaic {
                points: append(points),
            },
            Shape::Blur { points } => Shape::Blur {
                points: append(points),
            },
            Shape::Spotlight { from, .. } => Shape::Spotlight {
                from: *from,
                to: constrain_box(*from),
            },
            Shape::Text { .. } | Shape::Step { .. } => self.clone(),
        }
    }

    /// Whether the shape is worth keeping when the pointer is released: a
    /// click without a drag leaves nothing.
    pub fn is_visible(&self) -> bool {
        match self {
            Shape::Rectangle { from, to }
            | Shape::Ellipse { from, to }
            | Shape::Arrow { from, to }
            | Shape::Line { from, to } => from.distance(*to) >= 2.,
            Shape::Spotlight { from, to } => {
                (from.x - to.x).abs() >= 2. && (from.y - to.y).abs() >= 2.
            }
            Shape::Pen { points }
            | Shape::Marker { points }
            | Shape::Mosaic { points }
            | Shape::Blur { points } => !points.is_empty(),
            Shape::Text { content, .. } => !content.trim().is_empty(),
            Shape::Step { .. } => true,
        }
    }

    /// The same shape moved by `(dx, dy)`.
    pub fn translate(&self, dx: f32, dy: f32) -> Shape {
        let at = |point: &ScenePoint| ScenePoint::new(point.x + dx, point.y + dy);
        let all = |points: &Arc<[ScenePoint]>| points.iter().map(at).collect();
        match self {
            Shape::Rectangle { from, to } => Shape::Rectangle {
                from: at(from),
                to: at(to),
            },
            Shape::Ellipse { from, to } => Shape::Ellipse {
                from: at(from),
                to: at(to),
            },
            Shape::Arrow { from, to } => Shape::Arrow {
                from: at(from),
                to: at(to),
            },
            Shape::Line { from, to } => Shape::Line {
                from: at(from),
                to: at(to),
            },
            Shape::Pen { points } => Shape::Pen {
                points: all(points),
            },
            Shape::Marker { points } => Shape::Marker {
                points: all(points),
            },
            Shape::Mosaic { points } => Shape::Mosaic {
                points: all(points),
            },
            Shape::Blur { points } => Shape::Blur {
                points: all(points),
            },
            Shape::Spotlight { from, to } => Shape::Spotlight {
                from: at(from),
                to: at(to),
            },
            Shape::Text { origin, content } => Shape::Text {
                origin: at(origin),
                content: content.clone(),
            },
            Shape::Step { center, number } => Shape::Step {
                center: at(center),
                number: *number,
            },
        }
    }
}

/// Identifies an annotation across history snapshots, so rasterized text
/// and mosaic tiles are cached once per annotation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AnnotationId(pub u64);

/// One mark on the capture.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    id: AnnotationId,
    shape: Shape,
    /// In physical pixels.
    style: Style,
}

impl Annotation {
    pub fn new(id: AnnotationId, shape: Shape, style: Style) -> Self {
        Self { id, shape, style }
    }

    pub fn id(&self) -> AnnotationId {
        self.id
    }

    pub fn shape(&self) -> &Shape {
        &self.shape
    }

    pub fn style(&self) -> &Style {
        &self.style
    }

    pub fn with_shape(&self, shape: Shape) -> Self {
        Self {
            shape,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extend_constrains_boxes_and_angles() {
        let origin = ScenePoint::new(10., 10.);
        let Shape::Rectangle { to, .. } = Shape::start(Tool::Rectangle, origin)
            .unwrap()
            .extend(ScenePoint::new(40., 20.), true)
        else {
            unreachable!()
        };
        assert_eq!(to, ScenePoint::new(40., 40.), "shift makes a square");

        let Shape::Line { to, .. } = Shape::start(Tool::Line, origin)
            .unwrap()
            .extend(ScenePoint::new(110., 14.), true)
        else {
            unreachable!()
        };
        assert!((to.y - 10.).abs() < 0.001, "snaps to horizontal");
    }

    #[test]
    fn test_extend_skips_sub_pixel_points() {
        let shape = Shape::start(Tool::Pen, ScenePoint::new(0., 0.)).unwrap();
        let shape = shape.extend(ScenePoint::new(0.3, 0.3), false);
        let shape = shape.extend(ScenePoint::new(5., 5.), false);
        let Shape::Pen { points } = shape else {
            unreachable!()
        };
        assert_eq!(points.len(), 2);
    }

    #[test]
    fn test_click_without_drag_is_not_visible() {
        let point = ScenePoint::new(3., 3.);
        assert!(!Shape::start(Tool::Arrow, point).unwrap().is_visible());
        assert!(
            Shape::start(Tool::Pen, point).unwrap().is_visible(),
            "a dot"
        );
    }

    #[test]
    fn test_step_stroke_width() {
        let style = Style::default().with_stroke_width(STROKE_WIDTHS[0]);
        assert_eq!(
            style.step_stroke_width(true).stroke_width(),
            STROKE_WIDTHS[1]
        );
        assert_eq!(
            style.step_stroke_width(false).stroke_width(),
            STROKE_WIDTHS[0],
            "stays at the thinnest"
        );
    }
}
