//! What a capture session is doing, as plain data.
//!
//! The overlays turn pointer and key events into calls here and draw what
//! the state says; no GPUI type appears in this file, so every interaction
//! rule (selection, handles, drawing, the Escape order) is unit tested.
//!
//! A session moves through these steps:
//!
//! - **Selecting**: no selection yet. The window under the pointer is
//!   highlighted; a click takes it, a drag draws a rectangle.
//! - **Adjusting**: a selection exists and no tool is active. Its handles
//!   resize it, its body moves it, arrow keys nudge it.
//! - **Annotating**: a tool is active; drags inside the selection draw.
//!
//! Pressing on an annotation, with or without a tool, selects it and drags
//! move it; Delete removes it and picking a color recolors it.
//!
//! Escape steps back one layer at a time: a text being edited is committed,
//! a gesture in progress is cancelled, a selected annotation is deselected,
//! an active tool is put down, and only then is the session cancelled. A
//! secondary click steps from Adjusting back to Selecting, and from
//! Selecting out of the session.
//!
//! The selections of earlier captures can be recalled, newest first, while
//! nothing has been drawn, to capture the same region again.

use std::sync::Arc;

use crate::{
    geometry::{DisplayArea, Grip, PhysPoint, PhysRect, WindowSnapshot, grip_at, region_at},
    scene::{Annotation, AnnotationId, History, ScenePoint, Shape, Style, Tool, topmost_at},
};

/// How a session ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Copy,
    /// Save to the save folder without asking.
    Save,
    /// Ask where to save.
    SaveAs,
    Pin,
    /// Copy the text in the capture: QR code contents, or recognized words.
    CopyText,
    /// Scroll the content under the selection and join it into one image.
    ScrollCapture,
    Cancel,
}

/// What the overlay has to do after an input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    None,
    Finish(Outcome),
    /// Start editing a text annotation; the overlay shows a text field.
    EditText,
    /// Commit the text being edited with what the text field holds.
    CommitText,
}

/// A text annotation being typed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextDraft {
    origin: ScenePoint,
    /// In physical pixels of the display it is on.
    style: Style,
}

impl TextDraft {
    pub fn origin(&self) -> ScenePoint {
        self.origin
    }

    pub fn style(&self) -> &Style {
        &self.style
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Gesture {
    /// Dragging out a new selection on `display`.
    Creating {
        anchor: PhysPoint,
        current: PhysPoint,
        display: PhysRect,
    },
    /// Moving or resizing the selection.
    Gripping {
        grip: Grip,
        from: PhysPoint,
        start: PhysRect,
        display: PhysRect,
    },
    /// Drawing an annotation.
    Drawing { draft: Annotation },
    /// Dragging an annotation; its first move makes the undo step.
    Moving {
        from: ScenePoint,
        original: Arc<Annotation>,
        is_committed: bool,
    },
}

/// Handles within this many logical pixels of the pointer can be grabbed.
const GRIP_TOLERANCE: f32 = 6.;

/// A press and release closer than this many physical pixels is a click.
const CLICK_DISTANCE: i32 = 3;

#[derive(Debug)]
pub struct SessionState {
    displays: Vec<DisplayArea>,
    windows: Vec<WindowSnapshot>,
    pointer: Option<ScenePoint>,
    hover: Option<PhysRect>,
    selection: Option<PhysRect>,
    gesture: Option<Gesture>,
    tool: Option<Tool>,
    style: Style,
    history: History,
    text: Option<TextDraft>,
    /// The annotation picked for moving, deleting or restyling.
    selected: Option<AnnotationId>,
    next_id: u64,
    /// Selections of earlier captures, newest first.
    recent: Vec<PhysRect>,
    /// Which of `recent` the selection was recalled from.
    recalled: Option<usize>,
}

impl SessionState {
    /// A session over `displays`, offering `windows` (frontmost first) for
    /// automatic selection.
    pub fn new(displays: Vec<DisplayArea>, windows: Vec<WindowSnapshot>) -> Self {
        Self {
            displays,
            windows,
            pointer: None,
            hover: None,
            selection: None,
            gesture: None,
            tool: None,
            style: Style::default(),
            history: History::default(),
            text: None,
            selected: None,
            next_id: 1,
            recent: Vec::new(),
            recalled: None,
        }
    }

    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// The selections of earlier captures, newest first, to recall. Those
    /// on no display are dropped; the rest are cut to their display.
    pub fn with_recent_selections(mut self, recent: Vec<PhysRect>) -> Self {
        self.recent = recent
            .into_iter()
            .filter_map(|rect| {
                let display = self
                    .displays
                    .iter()
                    .find(|display| display.bounds().contains(rect.origin()))?;
                rect.intersect(&display.bounds())
            })
            .collect();
        self
    }

    /// The selection as drawn now, including one being dragged out.
    pub fn selection(&self) -> Option<PhysRect> {
        match &self.gesture {
            Some(Gesture::Creating {
                anchor, current, ..
            }) => Some(PhysRect::from_corners(*anchor, *current)),
            _ => self.selection,
        }
    }

    /// The region a click would select, while there is no selection.
    pub fn hover(&self) -> Option<PhysRect> {
        if self.selection.is_some() || self.gesture.is_some() {
            return None;
        }
        self.hover
    }

    pub fn pointer(&self) -> Option<ScenePoint> {
        self.pointer
    }

    pub fn tool(&self) -> Option<Tool> {
        self.tool
    }

    pub fn style(&self) -> &Style {
        &self.style
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// The annotation being drawn, not yet in history.
    pub fn draft(&self) -> Option<&Annotation> {
        match &self.gesture {
            Some(Gesture::Drawing { draft }) => Some(draft),
            _ => None,
        }
    }

    pub fn text_draft(&self) -> Option<&TextDraft> {
        self.text.as_ref()
    }

    /// The selected annotation as it is now.
    pub fn selected_annotation(&self) -> Option<&Arc<Annotation>> {
        self.history.current().get(self.selected?)
    }

    /// The annotation being dragged and how far, for drawing its old tile
    /// shifted instead of rebuilding it on every move.
    pub fn moving(&self) -> Option<(AnnotationId, f32, f32)> {
        let Some(Gesture::Moving { from, original, .. }) = &self.gesture else {
            return None;
        };
        let pointer = self.pointer?;
        Some((original.id(), pointer.x - from.x, pointer.y - from.y))
    }

    /// Whether a press would pick an annotation rather than draw or grip.
    pub fn is_over_annotation(&self) -> bool {
        if let Some(Gesture::Moving { .. }) = self.gesture {
            return true;
        }
        let (Some(selection), Some(pointer)) = (self.selection, self.pointer) else {
            return false;
        };
        self.gesture.is_none()
            && self.text.is_none()
            && selection.contains(physical(pointer))
            && self.annotation_at(pointer).is_some()
    }

    fn annotation_at(&self, point: ScenePoint) -> Option<Arc<Annotation>> {
        let tolerance = self.tolerance(physical(point)) as f32;
        topmost_at(self.history.current().annotations(), point, tolerance).cloned()
    }

    /// Whether the selection is settled: it exists and isn't being dragged.
    /// The toolbar shows only then.
    pub fn is_settled(&self) -> bool {
        self.selection.is_some()
            && !matches!(
                self.gesture,
                Some(Gesture::Creating { .. } | Gesture::Gripping { .. })
            )
    }

    /// The grip under the pointer, for the cursor shape.
    pub fn grip_at_pointer(&self) -> Option<Grip> {
        if self.tool.is_some() || self.text.is_some() || self.is_over_annotation() {
            return None;
        }
        if let Some(Gesture::Gripping { grip, .. }) = &self.gesture {
            return Some(*grip);
        }
        let selection = self.selection?;
        let pointer = self.pointer?;
        let point = physical(pointer);
        grip_at(&selection, point, self.tolerance(point))
    }

    pub fn display_at(&self, point: PhysPoint) -> Option<DisplayArea> {
        self.displays
            .iter()
            .find(|display| display.bounds().contains(point))
            .or_else(|| self.displays.first())
            .copied()
    }

    fn tolerance(&self, point: PhysPoint) -> i32 {
        let scale = self.display_at(point).map_or(1., |display| display.scale());
        (GRIP_TOLERANCE * scale).round() as i32
    }

    fn region_under(&self, point: PhysPoint, detect_windows: bool) -> Option<PhysRect> {
        let display = self.display_at(point)?.bounds();
        let region = detect_windows
            .then(|| region_at(&self.windows, point))
            .flatten()
            .and_then(|region| region.intersect(&display));
        Some(region.unwrap_or(display))
    }

    fn next_id(&mut self) -> AnnotationId {
        let id = AnnotationId(self.next_id);
        self.next_id += 1;
        id
    }

    /// The current style at the scale of the display under `point`.
    fn physical_style(&self, point: PhysPoint) -> Style {
        let scale = self.display_at(point).map_or(1., |display| display.scale());
        self.style.to_physical(scale)
    }

    pub fn pointer_down(&mut self, point: ScenePoint) -> Effect {
        self.pointer = Some(point);
        let at = physical(point);
        let Some(selection) = self.selection else {
            let Some(display) = self.display_at(at) else {
                return Effect::None;
            };
            self.gesture = Some(Gesture::Creating {
                anchor: at,
                current: at,
                display: display.bounds(),
            });
            return Effect::None;
        };
        // A resize handle wins over an annotation under it; then marks are
        // picked before anything is drawn.
        let is_on_handle = self.tool.is_none()
            && matches!(
                grip_at(&selection, at, self.tolerance(at)),
                Some(Grip::Handle(_))
            );
        if !is_on_handle
            && selection.contains(at)
            && let Some(annotation) = self.annotation_at(point)
        {
            self.selected = Some(annotation.id());
            self.gesture = Some(Gesture::Moving {
                from: point,
                original: annotation,
                is_committed: false,
            });
            return Effect::None;
        }
        self.selected = None;
        match self.tool {
            Some(tool) => {
                if !selection.contains(at) {
                    return Effect::None;
                }
                let style = self.physical_style(at);
                match tool {
                    Tool::Text => {
                        self.text = Some(TextDraft {
                            origin: point,
                            style,
                        });
                        Effect::EditText
                    }
                    Tool::Step => {
                        let number = self.history.current().next_step_number();
                        let id = self.next_id();
                        let step = Annotation::new(
                            id,
                            Shape::Step {
                                center: point,
                                number,
                            },
                            style,
                        );
                        self.history.commit(self.history.current().with(step));
                        Effect::None
                    }
                    tool => {
                        if let Some(shape) = Shape::start(tool, point) {
                            let id = self.next_id();
                            self.gesture = Some(Gesture::Drawing {
                                draft: Annotation::new(id, shape, style),
                            });
                        }
                        Effect::None
                    }
                }
            }
            None => {
                let Some(display) = self.display_at(selection.origin()) else {
                    return Effect::None;
                };
                if let Some(grip) = grip_at(&selection, at, self.tolerance(at)) {
                    self.gesture = Some(Gesture::Gripping {
                        grip,
                        from: at,
                        start: selection,
                        display: display.bounds(),
                    });
                } else if self.history.current().is_empty() {
                    // Outside an unmarked selection, a drag starts over.
                    if let Some(display) = self.display_at(at) {
                        self.gesture = Some(Gesture::Creating {
                            anchor: at,
                            current: at,
                            display: display.bounds(),
                        });
                    }
                }
                Effect::None
            }
        }
    }

    /// The pointer moved, with or without the primary button held.
    /// `is_constrained` (Shift) makes squares, circles and 45° lines.
    pub fn pointer_move(&mut self, point: ScenePoint, is_constrained: bool, detect_windows: bool) {
        self.pointer = Some(point);
        let at = physical(point);
        match &mut self.gesture {
            Some(Gesture::Creating {
                current, display, ..
            }) => {
                *current = display.clamp_point(at);
            }
            Some(Gesture::Gripping {
                grip,
                from,
                start,
                display,
            }) => {
                let moved = grip.drag(start, at.x - from.x, at.y - from.y);
                let fitted = match grip {
                    Grip::Body => moved.move_within(display),
                    Grip::Handle(_) => moved.intersect(display).unwrap_or(*start),
                };
                self.selection = Some(fitted);
            }
            Some(Gesture::Drawing { draft }) => {
                *draft = draft.with_shape(draft.shape().extend(point, is_constrained));
            }
            Some(Gesture::Moving {
                from,
                original,
                is_committed,
            }) => {
                let moved = original.with_shape(
                    original
                        .shape()
                        .translate(point.x - from.x, point.y - from.y),
                );
                let scene = self.history.current().replacing(moved);
                if *is_committed {
                    self.history.amend(scene);
                } else {
                    self.history.commit(scene);
                    *is_committed = true;
                }
            }
            None => {
                if self.selection.is_none() {
                    self.hover = self.region_under(at, detect_windows);
                }
            }
        }
    }

    pub fn pointer_up(&mut self, point: ScenePoint, detect_windows: bool) {
        self.pointer = Some(point);
        match self.gesture.take() {
            Some(Gesture::Creating {
                anchor, current, ..
            }) => {
                let rect = PhysRect::from_corners(anchor, current);
                self.selection = if rect.width < CLICK_DISTANCE && rect.height < CLICK_DISTANCE {
                    self.hover
                        .or_else(|| self.region_under(anchor, detect_windows))
                } else {
                    Some(rect)
                };
                self.hover = None;
            }
            Some(Gesture::Gripping { .. } | Gesture::Moving { .. }) => {}
            Some(Gesture::Drawing { draft }) => {
                if draft.shape().is_visible() {
                    self.history.commit(self.history.current().with(draft));
                }
                self.selected = None;
            }
            None => {}
        }
    }

    /// A double click inside the selection copies it, as in most capture tools.
    pub fn double_click(&mut self, point: ScenePoint) -> Effect {
        let inside = self
            .selection
            .is_some_and(|selection| selection.contains(physical(point)));
        if inside && self.tool.is_none() && self.text.is_none() {
            Effect::Finish(Outcome::Copy)
        } else {
            Effect::None
        }
    }

    /// A secondary click: cancel a gesture, or drop the selection, or leave.
    pub fn secondary_click(&mut self, detect_windows: bool) -> Effect {
        if self.text.take().is_some() {
            return Effect::None;
        }
        if let Some(gesture) = self.gesture.take() {
            self.restore(gesture);
            return Effect::None;
        }
        if self.selection.is_some() {
            self.selection = None;
            self.selected = None;
            self.tool = None;
            self.history = History::default();
            if let Some(pointer) = self.pointer {
                self.hover = self.region_under(physical(pointer), detect_windows);
            }
            return Effect::None;
        }
        Effect::Finish(Outcome::Cancel)
    }

    /// Escape: commit text, else cancel the gesture, else put the tool
    /// down, else cancel the session.
    pub fn escape(&mut self) -> Effect {
        if self.text.is_some() {
            return Effect::CommitText;
        }
        if let Some(gesture) = self.gesture.take() {
            self.restore(gesture);
            return Effect::None;
        }
        if self.selected.take().is_some() {
            return Effect::None;
        }
        if self.tool.take().is_some() {
            return Effect::None;
        }
        Effect::Finish(Outcome::Cancel)
    }

    fn restore(&mut self, gesture: Gesture) {
        match gesture {
            Gesture::Gripping { start, .. } => self.selection = Some(start),
            Gesture::Moving {
                is_committed: true, ..
            } => self.history.discard(),
            _ => {}
        }
    }

    /// Removes the selected annotation.
    pub fn delete_selected(&mut self) {
        if self.gesture.is_some() {
            return;
        }
        if let Some(id) = self.selected.take() {
            self.history.commit(self.history.current().without(id));
        }
    }

    /// Ends text editing, keeping `content` unless it is blank.
    pub fn commit_text(&mut self, content: &str) {
        let Some(draft) = self.text.take() else {
            return;
        };
        if content.trim().is_empty() {
            return;
        }
        let id = self.next_id();
        let annotation = Annotation::new(
            id,
            Shape::Text {
                origin: draft.origin,
                content: content.trim_end().into(),
            },
            draft.style,
        );
        self.history.commit(self.history.current().with(annotation));
    }

    /// Ends the session with `outcome`, which needs a selection unless it
    /// is a cancel.
    pub fn finish(&self, outcome: Outcome) -> Effect {
        if outcome != Outcome::Cancel && (self.selection.is_none() || self.gesture.is_some()) {
            return Effect::None;
        }
        Effect::Finish(outcome)
    }

    /// Picks up `tool`, or puts it down if it is already in hand.
    pub fn toggle_tool(&mut self, tool: Tool) {
        if self.selection.is_none() || self.gesture.is_some() {
            return;
        }
        self.tool = if self.tool == Some(tool) {
            None
        } else {
            Some(tool)
        };
    }

    /// The style for what is drawn next, and for the selected annotation.
    pub fn set_style(&mut self, style: Style) {
        self.style = style;
        if self.gesture.is_some() {
            return;
        }
        let Some(selected) = self.selected_annotation().cloned() else {
            return;
        };
        let (at, _) = crate::scene::bounds(&selected);
        let restyled = Annotation::new(
            selected.id(),
            selected.shape().clone(),
            self.physical_style(physical(at)),
        );
        if restyled != *selected {
            self.history
                .commit(self.history.current().replacing(restyled));
        }
    }

    /// Moves the selection by whole pixels, staying on its display.
    pub fn nudge(&mut self, dx: i32, dy: i32) {
        if self.gesture.is_some() {
            return;
        }
        let Some(selection) = self.selection else {
            return;
        };
        let Some(display) = self.display_at(selection.origin()) else {
            return;
        };
        self.selection = Some(selection.translate(dx, dy).move_within(&display.bounds()));
    }

    /// Grows or shrinks the selection from its bottom-right corner.
    pub fn resize(&mut self, dw: i32, dh: i32) {
        if self.gesture.is_some() {
            return;
        }
        let Some(selection) = self.selection else {
            return;
        };
        let Some(display) = self.display_at(selection.origin()) else {
            return;
        };
        let bounds = display.bounds();
        let width = (selection.width + dw).clamp(1, bounds.right() - selection.x);
        let height = (selection.height + dh).clamp(1, bounds.bottom() - selection.y);
        self.selection = Some(PhysRect::new(selection.x, selection.y, width, height));
    }

    /// Selects the whole display under the pointer.
    pub fn select_display(&mut self) {
        if self.gesture.is_some() || !self.history.current().is_empty() {
            return;
        }
        let at = self.pointer.map(physical).unwrap_or_default();
        if let Some(display) = self.display_at(at) {
            self.selection = Some(display.bounds());
            self.hover = None;
        }
    }

    /// Adds the controls found in the window at `window_ix` (in the order
    /// the session was given), and updates what the pointer highlights.
    pub fn add_window_parts(&mut self, window_ix: usize, parts: Vec<PhysRect>) {
        let Some(window) = self.windows.get_mut(window_ix) else {
            return;
        };
        window.add_parts(parts);
        if self.selection.is_none()
            && self.gesture.is_none()
            && let Some(pointer) = self.pointer
        {
            self.hover = self.region_under(physical(pointer), true);
        }
    }

    /// Selects the region of an earlier capture: an older one, or a newer
    /// one again. Only while nothing has been drawn on the selection.
    pub fn recall_selection(&mut self, older: bool) {
        if self.gesture.is_some() || self.text.is_some() || !self.history.current().is_empty() {
            return;
        }
        let ix = match (self.recalled, older) {
            (None, true) => 0,
            (Some(ix), true) => ix + 1,
            (Some(ix), false) if ix > 0 => ix - 1,
            _ => return,
        };
        let Some(rect) = self.recent.get(ix) else {
            return;
        };
        self.selection = Some(*rect);
        self.recalled = Some(ix);
        self.hover = None;
    }

    pub fn undo(&mut self) {
        if self.gesture.is_none() && self.text.is_none() {
            self.history.undo();
        }
    }

    pub fn redo(&mut self) {
        if self.gesture.is_none() && self.text.is_none() {
            self.history.redo();
        }
    }
}

/// The pixel a point falls in.
pub fn physical(point: ScenePoint) -> PhysPoint {
    PhysPoint::new(point.x.floor() as i32, point.y.floor() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Handle;

    const DISPLAY: PhysRect = PhysRect::new(0, 0, 1000, 800);
    const SECOND: PhysRect = PhysRect::new(1000, 0, 2000, 1600);

    fn state() -> SessionState {
        SessionState::new(
            vec![DisplayArea::new(DISPLAY, 1.), DisplayArea::new(SECOND, 2.)],
            vec![WindowSnapshot::new(PhysRect::new(100, 100, 300, 200))],
        )
    }

    fn at(x: f32, y: f32) -> ScenePoint {
        ScenePoint::new(x, y)
    }

    fn drag(state: &mut SessionState, from: (f32, f32), to: (f32, f32)) {
        state.pointer_move(at(from.0, from.1), false, true);
        state.pointer_down(at(from.0, from.1));
        state.pointer_move(at(to.0, to.1), false, true);
        state.pointer_up(at(to.0, to.1), true);
    }

    fn selected() -> SessionState {
        let mut state = state();
        drag(&mut state, (10., 10.), (210., 110.));
        state
    }

    #[test]
    fn test_hover_then_click_selects_window() {
        let mut state = state();
        state.pointer_move(at(150., 150.), false, true);
        assert_eq!(state.hover(), Some(PhysRect::new(100, 100, 300, 200)));
        state.pointer_down(at(150., 150.));
        state.pointer_up(at(151., 150.), true);
        assert_eq!(state.selection(), Some(PhysRect::new(100, 100, 300, 200)));
        assert!(state.is_settled());
    }

    #[test]
    fn test_click_on_desktop_selects_display() {
        let mut state = state();
        state.pointer_move(at(1500., 500.), false, true);
        assert_eq!(state.hover(), Some(SECOND));
        state.pointer_down(at(1500., 500.));
        state.pointer_up(at(1500., 500.), true);
        assert_eq!(state.selection(), Some(SECOND));
    }

    #[test]
    fn test_without_detection_hover_is_display() {
        let mut state = state();
        state.pointer_move(at(150., 150.), false, false);
        assert_eq!(state.hover(), Some(DISPLAY));
    }

    #[test]
    fn test_drag_creates_selection_clamped_to_start_display() {
        let mut state = state();
        state.pointer_down(at(900., 700.));
        state.pointer_move(at(1200., 900.), false, true);
        assert_eq!(
            state.selection(),
            Some(PhysRect::new(900, 700, 100, 100)),
            "shown while dragging, cut at the display edge"
        );
        assert!(!state.is_settled());
        state.pointer_up(at(1200., 900.), true);
        assert_eq!(state.selection(), Some(PhysRect::new(900, 700, 100, 100)));
    }

    #[test]
    fn test_handle_resize_and_body_move() {
        let mut state = selected();
        assert_eq!(state.selection(), Some(PhysRect::new(10, 10, 200, 100)));
        drag(&mut state, (210., 110.), (260., 130.));
        assert_eq!(state.selection(), Some(PhysRect::new(10, 10, 250, 120)));
        drag(&mut state, (100., 50.), (-500., 50.));
        assert_eq!(
            state.selection(),
            Some(PhysRect::new(0, 10, 250, 120)),
            "a moved selection stays on its display"
        );
    }

    #[test]
    fn test_grip_at_pointer() {
        let mut state = selected();
        state.pointer_move(at(209., 111.), false, true);
        assert_eq!(
            state.grip_at_pointer(),
            Some(Grip::Handle(Handle::BottomRight))
        );
        state.toggle_tool(Tool::Pen);
        assert_eq!(state.grip_at_pointer(), None, "a tool owns the pointer");
    }

    #[test]
    fn test_drawing_commits_and_undoes() {
        let mut state = selected();
        state.toggle_tool(Tool::Rectangle);
        drag(&mut state, (20., 20.), (60., 60.));
        assert_eq!(state.history().current().annotations().len(), 1);
        drag(&mut state, (30., 30.), (30., 30.));
        assert_eq!(
            state.history().current().annotations().len(),
            1,
            "a click draws nothing"
        );
        state.pointer_down(at(500., 500.));
        assert!(
            state.draft().is_none(),
            "outside the selection draws nothing"
        );
        state.undo();
        assert!(state.history().current().is_empty());
        state.redo();
        assert_eq!(state.history().current().annotations().len(), 1);
    }

    #[test]
    fn test_style_is_scaled_to_display() {
        let mut state = state();
        drag(&mut state, (1100., 100.), (1500., 500.));
        state.toggle_tool(Tool::Line);
        state.pointer_down(at(1200., 200.));
        let width = state.draft().unwrap().style().stroke_width();
        assert_eq!(width, Style::default().stroke_width() * 2.);
    }

    #[test]
    fn test_step_numbers_count_up() {
        let mut state = selected();
        state.toggle_tool(Tool::Step);
        state.pointer_down(at(20., 20.));
        state.pointer_down(at(40., 40.));
        let numbers: Vec<u32> = state
            .history()
            .current()
            .annotations()
            .iter()
            .filter_map(|annotation| match annotation.shape() {
                Shape::Step { number, .. } => Some(*number),
                _ => None,
            })
            .collect();
        assert_eq!(numbers, [1, 2]);
    }

    #[test]
    fn test_text_editing() {
        let mut state = selected();
        state.toggle_tool(Tool::Text);
        assert_eq!(state.pointer_down(at(20., 20.)), Effect::EditText);
        assert_eq!(
            state.escape(),
            Effect::CommitText,
            "escape commits text first"
        );
        state.commit_text("Hello\n");
        assert!(state.text_draft().is_none());
        assert!(matches!(
            state.history().current().annotations()[0].shape(),
            Shape::Text { content, .. } if &**content == "Hello"
        ));

        state.pointer_down(at(30., 30.));
        state.commit_text("   ");
        assert_eq!(
            state.history().current().annotations().len(),
            1,
            "blank text is dropped"
        );
    }

    #[test]
    fn test_escape_order() {
        let mut state = selected();
        state.toggle_tool(Tool::Pen);
        state.pointer_down(at(20., 20.));
        state.pointer_move(at(40., 40.), false, true);
        assert_eq!(state.escape(), Effect::None, "cancels the stroke");
        assert!(state.draft().is_none());
        assert_eq!(state.tool(), Some(Tool::Pen));
        assert_eq!(state.escape(), Effect::None, "puts the tool down");
        assert_eq!(state.tool(), None);
        assert_eq!(state.escape(), Effect::Finish(Outcome::Cancel));
    }

    #[test]
    fn test_escape_restores_a_dragged_selection() {
        let mut state = selected();
        state.pointer_down(at(100., 50.));
        state.pointer_move(at(150., 80.), false, true);
        state.escape();
        assert_eq!(state.selection(), Some(PhysRect::new(10, 10, 200, 100)));
    }

    #[test]
    fn test_secondary_click_steps_back() {
        let mut state = selected();
        state.toggle_tool(Tool::Rectangle);
        drag(&mut state, (20., 20.), (60., 60.));
        assert_eq!(state.secondary_click(true), Effect::None);
        assert_eq!(state.selection(), None);
        assert!(state.history().current().is_empty());
        assert_eq!(state.tool(), None);
        assert_eq!(state.secondary_click(true), Effect::Finish(Outcome::Cancel));
    }

    #[test]
    fn test_finish_needs_selection() {
        let state = state();
        assert_eq!(state.finish(Outcome::Copy), Effect::None);
        assert_eq!(
            state.finish(Outcome::Cancel),
            Effect::Finish(Outcome::Cancel)
        );
        assert_eq!(
            selected().finish(Outcome::Pin),
            Effect::Finish(Outcome::Pin)
        );
    }

    #[test]
    fn test_double_click_copies() {
        let mut state = selected();
        assert_eq!(
            state.double_click(at(50., 50.)),
            Effect::Finish(Outcome::Copy)
        );
        assert_eq!(state.double_click(at(500., 500.)), Effect::None);
    }

    #[test]
    fn test_nudge_and_resize_keep_to_display() {
        let mut state = selected();
        state.nudge(-100, 0);
        assert_eq!(state.selection(), Some(PhysRect::new(0, 10, 200, 100)));
        state.resize(5, -200);
        assert_eq!(state.selection(), Some(PhysRect::new(0, 10, 205, 1)));
        state.resize(5000, 0);
        assert_eq!(state.selection().unwrap().right(), DISPLAY.right());
    }

    #[test]
    fn test_pick_move_and_delete_annotation() {
        let mut state = selected();
        state.toggle_tool(Tool::Rectangle);
        drag(&mut state, (20., 20.), (60., 60.));
        assert_eq!(state.history().current().annotations().len(), 1);

        // Pressing on the rectangle's edge picks it instead of drawing.
        drag(&mut state, (20., 40.), (30., 50.));
        assert_eq!(state.history().current().annotations().len(), 1);
        let moved = state.selected_annotation().unwrap().clone();
        assert!(matches!(
            moved.shape(),
            Shape::Rectangle { from, .. } if *from == ScenePoint::new(30., 30.)
        ));
        state.undo();
        assert!(
            matches!(
                state.history().current().annotations()[0].shape(),
                Shape::Rectangle { from, .. } if *from == ScenePoint::new(20., 20.)
            ),
            "one undo step for the whole move"
        );
        state.redo();

        state.set_style(state.style().with_color(crate::scene::PALETTE[4]));
        assert_eq!(
            state.selected_annotation().unwrap().style().color(),
            crate::scene::PALETTE[4]
        );

        state.delete_selected();
        assert!(state.history().current().is_empty());
        state.undo();
        assert_eq!(state.history().current().annotations().len(), 1);
    }

    #[test]
    fn test_escape_cancels_a_move_then_deselects() {
        let mut state = selected();
        state.toggle_tool(Tool::Line);
        drag(&mut state, (20., 20.), (100., 20.));
        state.pointer_down(at(50., 20.));
        state.pointer_move(at(50., 80.), false, true);
        assert_eq!(state.escape(), Effect::None);
        assert!(
            matches!(
                state.history().current().annotations()[0].shape(),
                Shape::Line { from, .. } if from.y == 20.
            ),
            "the move is undone"
        );
        assert!(!state.history().is_redoable());
        assert!(state.selected_annotation().is_some());
        assert_eq!(state.escape(), Effect::None, "deselects");
        assert!(state.selected_annotation().is_none());
        assert_eq!(state.tool(), Some(Tool::Line));
    }

    #[test]
    fn test_select_display() {
        let mut state = state();
        state.pointer_move(at(1500., 10.), false, true);
        state.select_display();
        assert_eq!(state.selection(), Some(SECOND));
    }

    #[test]
    fn test_controls_found_later_are_highlighted() {
        let mut state = state();
        state.pointer_move(at(150., 150.), false, true);
        assert_eq!(state.hover(), Some(PhysRect::new(100, 100, 300, 200)));
        state.add_window_parts(0, vec![PhysRect::new(140, 140, 40, 20)]);
        assert_eq!(
            state.hover(),
            Some(PhysRect::new(140, 140, 40, 20)),
            "the control under the pointer is highlighted at once"
        );
        state.add_window_parts(7, vec![PhysRect::new(0, 0, 10, 10)]);
    }

    #[test]
    fn test_recall_selection() {
        let recent = vec![
            PhysRect::new(20, 20, 50, 50),
            PhysRect::new(5000, 0, 10, 10),
            PhysRect::new(1900, 1500, 500, 500),
        ];
        let mut state = state().with_recent_selections(recent);
        state.recall_selection(false);
        assert_eq!(state.selection(), None, "nothing newer than none");
        state.recall_selection(true);
        assert_eq!(state.selection(), Some(PhysRect::new(20, 20, 50, 50)));
        state.recall_selection(true);
        assert_eq!(
            state.selection(),
            Some(PhysRect::new(1900, 1500, 500, 100)),
            "skips a region off every display and cuts one to its display"
        );
        state.recall_selection(true);
        assert_eq!(state.selection(), Some(PhysRect::new(1900, 1500, 500, 100)));
        state.recall_selection(false);
        assert_eq!(state.selection(), Some(PhysRect::new(20, 20, 50, 50)));

        state.toggle_tool(Tool::Rectangle);
        drag(&mut state, (25., 25.), (60., 60.));
        state.recall_selection(true);
        assert_eq!(
            state.selection(),
            Some(PhysRect::new(20, 20, 50, 50)),
            "a marked selection is kept"
        );
    }

    #[test]
    fn test_drag_outside_unmarked_selection_starts_over() {
        let mut state = selected();
        drag(&mut state, (500., 500.), (600., 600.));
        assert_eq!(state.selection(), Some(PhysRect::new(500, 500, 100, 100)));

        state.toggle_tool(Tool::Rectangle);
        drag(&mut state, (510., 510.), (550., 550.));
        state.toggle_tool(Tool::Rectangle);
        drag(&mut state, (800., 100.), (900., 200.));
        assert_eq!(
            state.selection(),
            Some(PhysRect::new(500, 500, 100, 100)),
            "a marked selection is kept"
        );
    }
}
