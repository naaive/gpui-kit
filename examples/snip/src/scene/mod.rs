//! The annotation document: shapes, their outlines and undo history.
//!
//! Pure data with no GPUI dependency, so every rule here is unit tested.

mod annotation;
mod history;
mod hit;
mod outline;

pub use annotation::{
    Annotation, AnnotationId, Color, FONT_SIZES, PALETTE, STROKE_WIDTHS, ScenePoint, Shape, Style,
    Tool,
};
pub use history::{History, Scene};
pub use hit::{bounds, topmost_at};
pub use outline::{Figure, Paint, PathOp, mosaic_width, outline};
