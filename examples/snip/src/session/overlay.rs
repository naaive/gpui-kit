//! One display's overlay: its frozen frame, the selection, the annotations
//! and the toolbar, in a borderless window covering the display.

use gpui_kit::component::{ActiveTheme as _, h_flex, input::Textarea, v_flex};
use gpui_kit::{
    App, BorderStyle, Bounds, ContentMask, Context, CursorStyle, DispatchPhase, Entity,
    FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
    Styled as _, Subscription, Window, canvas, div, fill, hsla, outline, point,
    prelude::FluentBuilder as _, px, size,
};

use super::{
    CONTEXT, CaptureSession, ColorNotation, SessionEvent,
    machine::{Effect, Outcome, SessionState},
    paint::{hsla as annotation_color, paint_rectangle_draft, paint_sprite},
    toolbar,
};
use crate::{
    geometry::{DisplayArea, Grip, Handle},
    scene::{ScenePoint, Tool},
};

/// How far outside the selection is darkened. The frozen frame is the
/// user's content, not interface chrome, so this treatment of it is a
/// fixed value rather than a theme color, in both light and dark themes.
fn scrim() -> Hsla {
    hsla(0., 0., 0., 0.45)
}

/// Side of a resize handle, in logical pixels.
const HANDLE_SIZE: f32 = 7.;

/// Pixels the magnifier shows on each side of the one under the pointer.
const LOUPE_RADIUS: i32 = 7;
/// Logical size of one magnified pixel.
const LOUPE_CELL: f32 = 8.;

pub struct Overlay {
    session: Entity<CaptureSession>,
    frame_ix: usize,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Overlay {
    pub fn new(
        session: Entity<CaptureSession>,
        frame_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.subscribe_in(&session, window, |this, _, event, window, cx| match event {
                SessionEvent::TextEnded => window.focus(&this.focus_handle, cx),
            }),
        ];
        Self {
            session,
            frame_ix,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    fn area(&self, cx: &App) -> DisplayArea {
        self.session.read(cx).capture().frames()[self.frame_ix].area()
    }

    /// Runs `change` on the session state from this overlay's window.
    fn change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut SessionState, bool) -> Effect,
    ) {
        self.session
            .update(cx, |session, cx| session.update_state(window, cx, change));
    }

    fn finish(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        self.change(window, cx, |state, _| state.finish(outcome));
    }

    fn on_primary_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The overlay decides where focus goes: to itself, or to a text
        // field the click starts. GPUI's own focus-on-click would take it
        // back from the field.
        window.prevent_default();
        window.focus(&self.focus_handle, cx);
        let point = super::scene_point(&self.area(cx), event.position);
        let is_editing = self.session.read(cx).text_input().is_some();
        if is_editing {
            // A click away from the text being typed commits it.
            self.session
                .update(cx, |session, cx| session.commit_text(window, cx));
            return;
        }
        if event.click_count == 2 {
            self.change(window, cx, |state, _| state.double_click(point));
        } else {
            self.change(window, cx, |state, _| state.pointer_down(point));
        }
    }

    fn on_secondary_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.change(window, cx, |state, detect| state.secondary_click(detect));
    }

    fn nudge(&mut self, dx: i32, dy: i32, window: &mut Window, cx: &mut Context<Self>) {
        self.change(window, cx, |state, _| {
            state.nudge(dx, dy);
            Effect::None
        });
    }

    fn resize(&mut self, dw: i32, dh: i32, window: &mut Window, cx: &mut Context<Self>) {
        self.change(window, cx, |state, _| {
            state.resize(dw, dh);
            Effect::None
        });
    }

    fn set_tool(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, cx| session.set_tool(tool, window, cx));
    }

    fn copy_color(&mut self, notation: ColorNotation, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, cx| session.copy_color(notation, cx));
    }

    fn step_stroke(&mut self, wider: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.change(window, cx, |state, _| {
            let style = state.style().step_stroke_width(wider);
            state.set_style(style);
            Effect::None
        });
    }
}

impl Focusable for Overlay {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The cursor for what the pointer would do at its position.
fn cursor(state: &SessionState, is_editing: bool) -> CursorStyle {
    if is_editing {
        return CursorStyle::Arrow;
    }
    if state.is_over_annotation() {
        return CursorStyle::OpenHand;
    }
    match state.tool() {
        Some(Tool::Text) => return CursorStyle::IBeam,
        Some(_) => return CursorStyle::Crosshair,
        None => {}
    }
    match state.grip_at_pointer() {
        Some(Grip::Body) => CursorStyle::ClosedHand,
        Some(Grip::Handle(handle)) if handle.is_diagonal_down() => {
            CursorStyle::ResizeUpLeftDownRight
        }
        Some(Grip::Handle(handle)) if handle.is_diagonal_up() => CursorStyle::ResizeUpRightDownLeft,
        Some(Grip::Handle(handle)) if handle.is_vertical() => CursorStyle::ResizeUpDown,
        Some(Grip::Handle(_)) => CursorStyle::ResizeLeftRight,
        None => CursorStyle::Crosshair,
    }
}

/// Logs gaps between painted frames long enough to be seen as stutter.
fn log_frame_gap() {
    use std::sync::Mutex;
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let now = std::time::Instant::now();
    if let Ok(mut last) = LAST.lock() {
        if let Some(previous) = last.replace(now) {
            let gap = now - previous;
            if gap.as_millis() >= 1000 {
                return;
            }
            if gap.as_millis() >= 25 {
                tracing::debug!("{gap:?} between overlay frames");
            } else {
                tracing::trace!("{gap:?} between overlay frames");
            }
        }
    }
}

/// Darkens `bounds` except `hole`.
fn paint_scrim(bounds: Bounds<Pixels>, hole: Option<Bounds<Pixels>>, window: &mut Window) {
    let Some(hole) = hole.and_then(|hole| {
        let hole = hole.intersect(&bounds);
        (hole.size.width > px(0.) && hole.size.height > px(0.)).then_some(hole)
    }) else {
        window.paint_quad(fill(bounds, scrim()));
        return;
    };
    let (left, top) = (bounds.origin.x, bounds.origin.y);
    let (right, bottom) = (bounds.right(), bounds.bottom());
    let rects = [
        Bounds::from_corners(point(left, top), point(right, hole.top())),
        Bounds::from_corners(point(left, hole.bottom()), point(right, bottom)),
        Bounds::from_corners(point(left, hole.top()), point(hole.left(), hole.bottom())),
        Bounds::from_corners(point(hole.right(), hole.top()), point(right, hole.bottom())),
    ];
    for rect in rects {
        if rect.size.width > px(0.) && rect.size.height > px(0.) {
            window.paint_quad(fill(rect, scrim()));
        }
    }
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Over a backdrop the overlay has no Root to set these.
        window.set_rem_size(cx.theme().font_size);
        // Tiles replaced since the last frame leave the GPU atlas.
        for image in self
            .session
            .update(cx, |session, _| session.take_stale_images())
        {
            window.drop_image(image).ok();
        }
        let render_started = std::time::Instant::now();
        let session = self.session.read(cx);
        let frame = session.capture().frames()[self.frame_ix].clone();
        let area = frame.area();
        let display = frame.bounds();
        let state = session.state();
        let theme = cx.theme();
        // The selection is the selected region of the screen, drawn over
        // whatever was captured; the selection color reads on any of it.
        let accent = theme.selection.opacity(1.);
        let handle_border = theme.background;

        let to_local = |rect| super::logical_bounds(&area, rect);
        let selection = state
            .selection()
            .filter(|selection| display.contains(selection.origin()));
        let local_selection = selection.map(to_local);
        let hover = state
            .hover()
            .filter(|hover| display.contains(hover.origin()))
            .map(to_local);
        let is_editing = session.text_input().is_some();
        let show_handles = state.is_settled() && state.tool().is_none() && !is_editing;
        let handles: Vec<Point<Pixels>> = match (show_handles, selection) {
            (true, Some(selection)) => Handle::ALL
                .iter()
                .map(|handle| {
                    let position = handle.position(&selection);
                    super::logical_point(
                        &area,
                        ScenePoint::new(position.x as f32, position.y as f32),
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        let image = session.image(self.frame_ix).cloned();
        let picked = state.selected_annotation().map(|annotation| {
            let (min, max) = crate::scene::bounds(annotation);
            let min = super::logical_point(&area, min);
            let max = super::logical_point(&area, max);
            Bounds::from_corners(min, max)
        });
        let draft = state.draft().cloned();
        // The capture dimmed around spotlights, the one being drawn included.
        let spotlight_shade: Vec<Bounds<Pixels>> = match selection {
            Some(selection) => {
                let mut holes = crate::scene::spotlights(
                    state
                        .history()
                        .current()
                        .annotations()
                        .iter()
                        .map(|annotation| &**annotation),
                );
                holes.extend(crate::scene::spotlights(state.draft()));
                let min = ScenePoint::new(selection.x as f32, selection.y as f32);
                let max = ScenePoint::new(selection.right() as f32, selection.bottom() as f32);
                crate::scene::shaded(min, max, &holes)
                    .into_iter()
                    .map(|(min, max)| {
                        Bounds::from_corners(
                            super::logical_point(&area, min),
                            super::logical_point(&area, max),
                        )
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let sprites: Vec<_> = session
            .sprites()
            .map(|(sprite, shift)| (sprite.clone(), shift))
            .collect();
        let cursor = cursor(state, is_editing);

        let size_label = match (selection, local_selection) {
            (Some(selection), Some(local)) => {
                Some((format!("{} × {}", selection.width, selection.height), local))
            }
            _ => None,
        };
        let pointer_here = state
            .pointer()
            .filter(|pointer| display.contains(super::machine::physical(*pointer)));
        let show_loupe = session.is_showing_magnifier()
            && state.tool().is_none()
            && !is_editing
            && (state.selection().is_none() || !state.is_settled())
            && pointer_here.is_some();
        let loupe = show_loupe
            .then(|| loupe(&frame, pointer_here?, &area, cx))
            .flatten();
        let toolbar = match (state.is_settled(), local_selection) {
            (true, Some(local)) => {
                Some(toolbar::render(session, local, to_local(display).size, cx))
            }
            _ => None,
        };
        let text_field = session.text_input().cloned().and_then(|input| {
            let draft = state.text_draft()?;
            let origin = draft.origin();
            if !display.contains(super::machine::physical(origin)) {
                return None;
            }
            let position = super::logical_point(&area, origin);
            let font_size = area.to_logical_length(draft.style().font_size());
            let color = annotation_color(draft.style().color());
            Some(
                div()
                    .id("text-field")
                    .absolute()
                    .left(position.x)
                    .top(position.y)
                    .min_w(px(160.))
                    // Clicks in the field place the caret; only clicks
                    // elsewhere commit the text.
                    .occlude()
                    .child(
                        Textarea::new(&input)
                            .appearance(false)
                            // Typed text sits where the committed text will.
                            .p_0()
                            .text_size(px(font_size))
                            .text_color(color)
                            .w(px(480.)),
                    ),
            )
        });

        let session_entity = self.session.clone();
        let paint_area = area;
        let render_time = render_started.elapsed();
        if render_time.as_millis() >= 4 {
            tracing::debug!("overlay render took {render_time:?}");
        }
        div()
            .id("overlay")
            .track_focus(&self.focus_handle)
            .key_context(CONTEXT)
            .size_full()
            .relative()
            .font_family(theme.font_family.clone())
            .text_color(theme.foreground)
            .cursor(cursor)
            .on_action(cx.listener(|this, _: &super::Cancel, window, cx| {
                this.change(window, cx, |state, _| state.escape());
            }))
            .on_action(cx.listener(|this, _: &super::CommitText, window, cx| {
                this.session
                    .update(cx, |session, cx| session.commit_text(window, cx));
            }))
            .on_action(cx.listener(|this, _: &super::Copy, window, cx| {
                this.finish(Outcome::Copy, window, cx);
            }))
            .on_action(cx.listener(|this, _: &super::Save, window, cx| {
                this.finish(Outcome::Save, window, cx);
            }))
            .on_action(cx.listener(|this, _: &super::SaveAs, window, cx| {
                this.finish(Outcome::SaveAs, window, cx);
            }))
            .on_action(cx.listener(|this, _: &super::Pin, window, cx| {
                this.finish(Outcome::Pin, window, cx);
            }))
            .on_action(cx.listener(|this, _: &super::CopyText, window, cx| {
                this.finish(Outcome::CopyText, window, cx);
            }))
            .on_action(cx.listener(|this, _: &super::ScrollCapture, window, cx| {
                this.finish(Outcome::ScrollCapture, window, cx);
            }))
            .on_action(cx.listener(|this, _: &super::Undo, window, cx| {
                this.change(window, cx, |state, _| {
                    state.undo();
                    Effect::None
                });
            }))
            .on_action(cx.listener(|this, _: &super::Redo, window, cx| {
                this.change(window, cx, |state, _| {
                    state.redo();
                    Effect::None
                });
            }))
            .on_action(cx.listener(|this, _: &super::SelectDisplay, window, cx| {
                this.change(window, cx, |state, _| {
                    state.select_display();
                    Effect::None
                });
            }))
            .on_action(
                cx.listener(|this, _: &super::PreviousSelection, window, cx| {
                    this.change(window, cx, |state, _| {
                        state.recall_selection(true);
                        Effect::None
                    });
                }),
            )
            .on_action(cx.listener(|this, _: &super::NextSelection, window, cx| {
                this.change(window, cx, |state, _| {
                    state.recall_selection(false);
                    Effect::None
                });
            }))
            .on_action(
                cx.listener(|this, _: &super::NudgeLeft, window, cx| this.nudge(-1, 0, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &super::NudgeRight, window, cx| this.nudge(1, 0, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &super::NudgeUp, window, cx| this.nudge(0, -1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &super::NudgeDown, window, cx| this.nudge(0, 1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &super::NudgeLeftFar, window, cx| {
                this.nudge(-10, 0, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::NudgeRightFar, window, cx| {
                this.nudge(10, 0, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &super::NudgeUpFar, window, cx| {
                    this.nudge(0, -10, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &super::NudgeDownFar, window, cx| {
                this.nudge(0, 10, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &super::GrowWidth, window, cx| this.resize(1, 0, window, cx)),
            )
            .on_action(cx.listener(|this, _: &super::ShrinkWidth, window, cx| {
                this.resize(-1, 0, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &super::GrowHeight, window, cx| {
                    this.resize(0, 1, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &super::ShrinkHeight, window, cx| {
                this.resize(0, -1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::CopyHexColor, _, cx| {
                this.copy_color(ColorNotation::Hex, cx);
            }))
            .on_action(cx.listener(|this, _: &super::CopyRgbColor, _, cx| {
                this.copy_color(ColorNotation::Rgb, cx);
            }))
            .on_action(cx.listener(|this, _: &super::WiderStroke, window, cx| {
                this.step_stroke(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &super::NarrowerStroke, window, cx| {
                this.step_stroke(false, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &super::DeleteAnnotation, window, cx| {
                    this.change(window, cx, |state, _| {
                        state.delete_selected();
                        Effect::None
                    });
                }),
            )
            .on_action(cx.listener(|this, _: &super::UseRectangle, window, cx| {
                this.set_tool(Tool::Rectangle, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UseEllipse, window, cx| {
                this.set_tool(Tool::Ellipse, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UseArrow, window, cx| {
                this.set_tool(Tool::Arrow, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UseLine, window, cx| {
                this.set_tool(Tool::Line, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UsePen, window, cx| {
                this.set_tool(Tool::Pen, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UseMarker, window, cx| {
                this.set_tool(Tool::Marker, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UseMosaic, window, cx| {
                this.set_tool(Tool::Mosaic, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UseBlur, window, cx| {
                this.set_tool(Tool::Blur, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UseSpotlight, window, cx| {
                this.set_tool(Tool::Spotlight, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UseText, window, cx| {
                this.set_tool(Tool::Text, window, cx)
            }))
            .on_action(cx.listener(|this, _: &super::UseStep, window, cx| {
                this.set_tool(Tool::Step, window, cx)
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_primary_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_secondary_down))
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |bounds, _, window, _| {
                        let paint_started = std::time::Instant::now();
                        log_frame_gap();
                        let origin = bounds.origin;
                        if let Some(image) = image {
                            window
                                .paint_image(bounds, bounds, Default::default(), image, 0, false)
                                .ok();
                        }
                        let offset = |local: Bounds<Pixels>| {
                            Bounds::new(
                                point(origin.x + local.origin.x, origin.y + local.origin.y),
                                local.size,
                            )
                        };
                        // Within one layer GPUI draws primitives by kind, images
                        // last; each stage gets a layer of its own so it lands on
                        // top of the frozen frame and of the stage before.
                        window.paint_layer(bounds, |window| {
                            paint_scrim(bounds, local_selection.or(hover).map(offset), window);
                        });
                        if let Some(local) = local_selection.map(offset) {
                            // Committed annotations are raster tiles (see
                            // `raster::sprite`); only the one being drawn is
                            // a path, so frames without a draft draw none.
                            window.paint_layer(bounds, |window| {
                                window.with_content_mask(
                                    Some(ContentMask { bounds: local }),
                                    |window| {
                                        for (sprite, shift) in &sprites {
                                            paint_sprite(
                                                sprite,
                                                *shift,
                                                &paint_area,
                                                origin,
                                                window,
                                            );
                                        }
                                    },
                                );
                            });
                            if let Some(draft) = &draft {
                                window.paint_layer(bounds, |window| {
                                    window.with_content_mask(
                                        Some(ContentMask { bounds: local }),
                                        |window| {
                                            paint_rectangle_draft(
                                                draft,
                                                &paint_area,
                                                origin,
                                                window,
                                            )
                                        },
                                    );
                                });
                            }
                        }
                        // Spotlights dim the marks around them too, as in export.
                        if !spotlight_shade.is_empty() {
                            let shade = annotation_color(crate::scene::SPOTLIGHT_SHADE);
                            window.paint_layer(bounds, |window| {
                                for shaded in &spotlight_shade {
                                    window.paint_quad(fill(offset(*shaded), shade));
                                }
                            });
                        }
                        window.paint_layer(bounds, |window| {
                            if let Some(local) = local_selection.map(offset) {
                                window.paint_quad(outline(local, accent, BorderStyle::Solid));
                            } else if let Some(hover) = hover.map(offset) {
                                window.paint_quad(
                                    outline(hover, accent, BorderStyle::Solid)
                                        .border_widths(px(2.)),
                                );
                            }
                            if let Some(picked) = picked {
                                window.paint_quad(outline(
                                    offset(picked).dilate(px(3.)),
                                    accent,
                                    BorderStyle::Dashed,
                                ));
                            }
                            for handle in &handles {
                                let half = px(HANDLE_SIZE / 2.);
                                window.paint_quad(
                                    fill(
                                        Bounds::new(
                                            point(
                                                origin.x + handle.x - half,
                                                origin.y + handle.y - half,
                                            ),
                                            size(px(HANDLE_SIZE), px(HANDLE_SIZE)),
                                        ),
                                        accent,
                                    )
                                    .border_widths(px(1.))
                                    .border_color(handle_border),
                                );
                            }
                        });

                        let frame_time = render_started.elapsed();
                        if frame_time.as_millis() >= 8 {
                            tracing::debug!("overlay frame cpu took {frame_time:?}");
                        }
                        let paint_time = paint_started.elapsed();
                        if paint_time.as_millis() >= 4 {
                            tracing::debug!("overlay paint took {paint_time:?}");
                        }
                        // Moves and releases anywhere in the window, including
                        // over the toolbar and past the display's edge while
                        // dragging, belong to the session.
                        let session = session_entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            tracing::trace!("pointer move");
                            let point = super::scene_point(&paint_area, event.position);
                            let is_constrained = event.modifiers.shift;
                            session.update(cx, |session, cx| {
                                session.update_state(window, cx, |state, detect| {
                                    state.pointer_move(point, is_constrained, detect);
                                    Effect::None
                                })
                            });
                        });
                        let session = session_entity.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                                return;
                            }
                            let point = super::scene_point(&paint_area, event.position);
                            session.update(cx, |session, cx| {
                                session.update_state(window, cx, |state, detect| {
                                    state.pointer_up(point, detect);
                                    Effect::None
                                })
                            });
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .when_some(size_label, |this, (label, local)| {
                let above = local.origin.y > px(28.);
                this.child(
                    div()
                        .absolute()
                        .left(local.origin.x)
                        .when(above, |this| this.top(local.origin.y - px(26.)))
                        .when(!above, |this| this.top(local.origin.y + px(4.)))
                        .when(!above, |this| this.ml(px(4.)))
                        .px_1p5()
                        .py_0p5()
                        .rounded(theme.radius)
                        .bg(theme.popover)
                        .text_color(theme.popover_foreground)
                        .text_xs()
                        .child(label),
                )
            })
            .when_some(toolbar, |this, toolbar| this.child(toolbar))
            .when_some(loupe, |this, loupe| this.child(loupe))
            .when_some(text_field, |this, field| this.child(field))
    }
}

/// The magnifier next to the pointer: the pixels around it, enlarged, with
/// the position and color of the one under it.
fn loupe(
    frame: &crate::capture::Frame,
    pointer: ScenePoint,
    area: &DisplayArea,
    cx: &App,
) -> Option<impl IntoElement> {
    let theme = cx.theme();
    let at = super::machine::physical(pointer);
    let center = frame.pixel(at)?;
    let span = (LOUPE_RADIUS * 2 + 1) as usize;
    let mut cells = Vec::with_capacity(span * span);
    for dy in -LOUPE_RADIUS..=LOUPE_RADIUS {
        for dx in -LOUPE_RADIUS..=LOUPE_RADIUS {
            cells.push(frame.pixel(crate::geometry::PhysPoint::new(at.x + dx, at.y + dy)));
        }
    }
    let side = LOUPE_CELL * span as f32;
    let position = super::logical_point(area, pointer);
    let display_size = super::logical_bounds(area, frame.bounds()).size;
    let (width, height) = (px(side + 2.), px(side + 76.));
    let gap = px(20.);
    let left = if position.x + gap + width > display_size.width {
        position.x - gap - width
    } else {
        position.x + gap
    };
    let top = if position.y + gap + height > display_size.height {
        position.y - gap - height
    } else {
        position.y + gap
    };
    let marker = theme.primary;
    Some(
        v_flex()
            .absolute()
            .left(left)
            .top(top)
            .w(width)
            .rounded(theme.radius)
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .shadow_md()
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |bounds, _, window, _| {
                        let origin = bounds.origin;
                        for (ix, cell) in cells.iter().enumerate() {
                            let x = (ix % span) as f32 * LOUPE_CELL;
                            let y = (ix / span) as f32 * LOUPE_CELL;
                            let cell_bounds = Bounds::new(
                                point(origin.x + px(x), origin.y + px(y)),
                                size(px(LOUPE_CELL), px(LOUPE_CELL)),
                            );
                            let color = cell.map_or(hsla(0., 0., 0., 1.), annotation_color);
                            window.paint_quad(fill(cell_bounds, color));
                        }
                        let center = LOUPE_RADIUS as f32 * LOUPE_CELL;
                        window.paint_quad(outline(
                            Bounds::new(
                                point(origin.x + px(center), origin.y + px(center)),
                                size(px(LOUPE_CELL), px(LOUPE_CELL)),
                            ),
                            marker,
                            BorderStyle::Solid,
                        ));
                    },
                )
                .w(px(side))
                .h(px(side)),
            )
            .child(
                v_flex()
                    .px_2()
                    .py_1p5()
                    .gap_0p5()
                    .text_xs()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(format!("{}, {}", at.x, at.y))
                            .child(
                                div()
                                    .size_3()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(annotation_color(center)),
                            ),
                    )
                    .child(ColorNotation::Hex.format(center))
                    .child(ColorNotation::Rgb.format(center))
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child("C copies, Shift-C as RGB"),
                    ),
            ),
    )
}
