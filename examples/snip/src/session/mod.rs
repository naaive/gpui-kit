//! A capture session: the frozen screen, one overlay window per display,
//! the selection and the annotations.
//!
//! [`CaptureSession`] is the one owner of a session's state. Each display
//! gets an [`overlay::Overlay`] window showing that display's frozen frame;
//! every overlay forwards its input here and draws from here, so a
//! selection dragged on one display is seen on all of them. The rules of
//! the interaction live in [`machine`], which has no GPUI dependency.

mod backdrop;
pub mod machine;
mod overlay;
mod paint;
mod toolbar;

use std::{collections::HashMap, sync::Arc};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, DisplayId, Entity, Focusable as _,
    KeyBinding, NoAction, Pixels, Point, RenderImage, Subscription, Window, WindowBounds,
    WindowKind, WindowOptions, actions, point, px, size,
};
use image::RgbaImage;
use smallvec::smallvec;

use self::machine::{Effect, Outcome, SessionState};
use crate::{
    app,
    capture::{CaptureSet, Frame},
    geometry::{DisplayArea, PhysRect},
    output,
    raster::{Tile, TileCache},
    scene::{AnnotationId, Color, ScenePoint, Shape, Tool},
};

pub(crate) const CONTEXT: &str = "SnipOverlay";
const TEXT_CONTEXT: &str = "SnipOverlay > Input";

actions!(
    snip_overlay,
    [
        /// Steps back one layer: commits text, cancels a drag, puts the tool
        /// down, or ends the session.
        Cancel,
        /// Copies the selection and ends the session.
        Copy,
        /// Saves the selection to the save folder and ends the session.
        Save,
        /// Asks where to save the selection.
        SaveAs,
        /// Pins the selection to the screen and ends the session.
        Pin,
        Undo,
        Redo,
        /// Selects the whole display under the pointer.
        SelectDisplay,
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
        NudgeLeftFar,
        NudgeRightFar,
        NudgeUpFar,
        NudgeDownFar,
        GrowWidth,
        ShrinkWidth,
        GrowHeight,
        ShrinkHeight,
        /// Copies the color under the magnifier as `#RRGGBB`.
        CopyHexColor,
        /// Copies the color under the magnifier as `rgb(r, g, b)`.
        CopyRgbColor,
        WiderStroke,
        NarrowerStroke,
        /// Removes the selected annotation.
        DeleteAnnotation,
        /// Ends text editing, keeping the text.
        CommitText,
        UseRectangle,
        UseEllipse,
        UseArrow,
        UseLine,
        UsePen,
        UseMarker,
        UseMosaic,
        UseText,
        UseStep,
    ]
);

/// The action that picks up `tool`, and its key.
pub(crate) fn tool_action(tool: Tool) -> (Box<dyn gpui_kit::Action>, &'static str) {
    match tool {
        Tool::Rectangle => (Box::new(UseRectangle), "r"),
        Tool::Ellipse => (Box::new(UseEllipse), "e"),
        Tool::Arrow => (Box::new(UseArrow), "a"),
        Tool::Line => (Box::new(UseLine), "l"),
        Tool::Pen => (Box::new(UsePen), "p"),
        Tool::Marker => (Box::new(UseMarker), "m"),
        Tool::Mosaic => (Box::new(UseMosaic), "x"),
        Tool::Text => (Box::new(UseText), "t"),
        Tool::Step => (Box::new(UseStep), "n"),
    }
}

pub fn init(cx: &mut App) {
    let context = Some(CONTEXT);
    // Inside the text field, letters are text.
    let text = Some(TEXT_CONTEXT);
    let mut bindings = vec![
        KeyBinding::new("escape", Cancel, context),
        KeyBinding::new("enter", Copy, context),
        KeyBinding::new("secondary-c", Copy, context),
        KeyBinding::new("secondary-s", Save, context),
        KeyBinding::new("secondary-shift-s", SaveAs, context),
        KeyBinding::new("f3", Pin, context),
        KeyBinding::new("secondary-t", Pin, context),
        KeyBinding::new("secondary-z", Undo, context),
        KeyBinding::new("secondary-shift-z", Redo, context),
        KeyBinding::new("secondary-a", SelectDisplay, context),
        KeyBinding::new("left", NudgeLeft, context),
        KeyBinding::new("right", NudgeRight, context),
        KeyBinding::new("up", NudgeUp, context),
        KeyBinding::new("down", NudgeDown, context),
        KeyBinding::new("shift-left", NudgeLeftFar, context),
        KeyBinding::new("shift-right", NudgeRightFar, context),
        KeyBinding::new("shift-up", NudgeUpFar, context),
        KeyBinding::new("shift-down", NudgeDownFar, context),
        KeyBinding::new("secondary-right", GrowWidth, context),
        KeyBinding::new("secondary-left", ShrinkWidth, context),
        KeyBinding::new("secondary-down", GrowHeight, context),
        KeyBinding::new("secondary-up", ShrinkHeight, context),
        KeyBinding::new("c", CopyHexColor, context),
        KeyBinding::new("shift-c", CopyRgbColor, context),
        KeyBinding::new("]", WiderStroke, context),
        KeyBinding::new("[", NarrowerStroke, context),
        KeyBinding::new("delete", DeleteAnnotation, context),
        KeyBinding::new("backspace", DeleteAnnotation, context),
        KeyBinding::new("escape", CommitText, text),
        // The field's own Enter lets the key go on to Copy.
        KeyBinding::new("enter", CommitText, text),
        KeyBinding::new("c", NoAction, text),
        KeyBinding::new("shift-c", NoAction, text),
        KeyBinding::new("]", NoAction, text),
        KeyBinding::new("[", NoAction, text),
    ];
    #[cfg(not(target_os = "macos"))]
    bindings.push(KeyBinding::new("ctrl-y", Redo, context));
    for binding in [
        tool_binding(Tool::Rectangle, UseRectangle),
        tool_binding(Tool::Ellipse, UseEllipse),
        tool_binding(Tool::Arrow, UseArrow),
        tool_binding(Tool::Line, UseLine),
        tool_binding(Tool::Pen, UsePen),
        tool_binding(Tool::Marker, UseMarker),
        tool_binding(Tool::Mosaic, UseMosaic),
        tool_binding(Tool::Text, UseText),
        tool_binding(Tool::Step, UseStep),
    ] {
        bindings.extend(binding);
    }
    cx.bind_keys(bindings);
}

/// The key of `tool` on the overlay, and nothing inside the text field.
fn tool_binding(tool: Tool, action: impl gpui_kit::Action) -> [KeyBinding; 2] {
    let (_, key) = tool_action(tool);
    [
        KeyBinding::new(key, action, Some(CONTEXT)),
        KeyBinding::new(key, NoAction, Some(TEXT_CONTEXT)),
    ]
}

/// Points per finished chunk of a freehand draft.
const DRAFT_CHUNK_POINTS: usize = 24;

/// The finished chunks of a freehand draft.
#[derive(Default)]
struct DraftChunks {
    id: Option<AnnotationId>,
    start: usize,
    sprites: Vec<TileSprite>,
}

/// The GPU copy of a tile, kept while the tile's pixels are unchanged.
#[derive(Clone)]
pub(crate) struct TileSprite {
    tile: Tile,
    image: Arc<RenderImage>,
}

impl TileSprite {
    fn new(tile: Tile) -> Self {
        let image = render_image(tile.image());
        Self { tile, image }
    }

    pub(crate) fn tile(&self) -> &Tile {
        &self.tile
    }

    pub(crate) fn image(&self) -> &Arc<RenderImage> {
        &self.image
    }
}

/// A straight-alpha RGBA image as the BGRA image GPUI draws.
pub(crate) fn render_image(image: &RgbaImage) -> Arc<RenderImage> {
    let mut bgra = image.clone();
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(smallvec![image::Frame::new(bgra)]))
}

/// An open overlay window.
struct OverlayWindow {
    handle: AnyWindowHandle,
}

/// Which way a picked color is copied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ColorNotation {
    Hex,
    Rgb,
}

impl ColorNotation {
    pub(crate) fn format(self, color: Color) -> String {
        match self {
            Self::Hex => color.hex(),
            Self::Rgb => format!("rgb({}, {}, {})", color.r, color.g, color.b),
        }
    }
}

pub struct CaptureSession {
    capture: CaptureSet,
    state: SessionState,
    /// The frozen frames as GPUI images, by frame index.
    images: Vec<Arc<RenderImage>>,
    /// Text, number and mosaic tiles, handed to export.
    tiles: TileCache,
    /// Whole-annotation tiles, which the overlay draws.
    sprite_tiles: TileCache,
    sprites: HashMap<AnnotationId, TileSprite>,
    /// Images no longer drawn, to drop from the GPU atlas.
    stale_images: Vec<Arc<RenderImage>>,
    /// The draft's finished stroke chunks, and where the next one starts.
    draft_chunks: DraftChunks,
    /// The draft's latest part, rebuilt as it changes.
    draft_sprite: Option<TileSprite>,
    overlays: Vec<OverlayWindow>,
    /// The windows showing the frozen frames under the overlays, where the
    /// frames aren't drawn by the overlays themselves.
    backdrops: Vec<AnyWindowHandle>,
    has_backdrops: bool,
    text_input: Option<Entity<InputState>>,
    is_detecting_windows: bool,
    is_showing_magnifier: bool,
    is_finished: bool,
    _subscriptions: Vec<Subscription>,
}

impl CaptureSession {
    fn new(capture: CaptureSet, images: Vec<Arc<RenderImage>>, cx: &App) -> Self {
        let settings = app::settings(cx);
        let displays = capture.frames().iter().map(Frame::area).collect();
        let mut state = SessionState::new(displays, capture.windows().to_vec())
            .with_style(settings.annotation_style());
        if let Some(pointer) = capture.pointer() {
            state.pointer_move(
                ScenePoint::new(pointer.x as f32 + 0.5, pointer.y as f32 + 0.5),
                false,
                settings.is_detecting_windows(),
            );
        }
        Self {
            capture,
            state,
            images,
            tiles: TileCache::default(),
            sprite_tiles: TileCache::new(crate::raster::sprite),
            sprites: HashMap::new(),
            stale_images: Vec::new(),
            draft_chunks: DraftChunks::default(),
            draft_sprite: None,
            overlays: Vec::new(),
            backdrops: Vec::new(),
            has_backdrops: false,
            text_input: None,
            is_detecting_windows: settings.is_detecting_windows(),
            is_showing_magnifier: settings.is_showing_magnifier(),
            is_finished: false,
            _subscriptions: Vec::new(),
        }
    }

    pub(crate) fn state(&self) -> &SessionState {
        &self.state
    }

    pub(crate) fn capture(&self) -> &CaptureSet {
        &self.capture
    }

    /// The frozen frame an overlay draws itself; none where a backdrop
    /// window shows it.
    pub(crate) fn image(&self, frame_ix: usize) -> Option<&Arc<RenderImage>> {
        if self.has_backdrops {
            return None;
        }
        self.images.get(frame_ix)
    }

    pub(crate) fn text_input(&self) -> Option<&Entity<InputState>> {
        self.text_input.as_ref()
    }

    pub(crate) fn is_showing_magnifier(&self) -> bool {
        self.is_showing_magnifier
    }

    /// The tiles of committed annotations and of the draft, bottom to top,
    /// each with the physical offset to draw it at: a dragged annotation
    /// keeps its tile, shifted, until it is dropped.
    pub(crate) fn sprites(&self) -> impl Iterator<Item = (&TileSprite, (f32, f32))> {
        let moving = self.state.moving();
        self.state
            .history()
            .current()
            .annotations()
            .iter()
            .filter_map(move |annotation| {
                let sprite = self.sprites.get(&annotation.id())?;
                let offset = match moving {
                    Some((id, dx, dy)) if id == annotation.id() => (dx, dy),
                    _ => (0., 0.),
                };
                Some((sprite, offset))
            })
            .chain(
                self.draft_chunks
                    .sprites
                    .iter()
                    .chain(self.draft_sprite.as_ref())
                    .map(|sprite| (sprite, (0., 0.))),
            )
    }

    /// Takes the images no longer drawn, for the window to free.
    pub(crate) fn take_stale_images(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.stale_images)
    }

    /// The frame annotations are drawn on: the one under the selection.
    fn selection_frame(&self) -> Option<&Frame> {
        let selection = self.state.selection()?;
        self.capture.frame_at(selection.origin())
    }

    /// Brings the GPU tiles up to date with the scene and the draft.
    fn refresh_sprites(&mut self) {
        let frame = self.selection_frame().cloned();
        let annotations = self.state.history().current().annotations().to_vec();
        self.tiles.retain(&annotations);
        self.sprite_tiles.retain(&annotations);
        let moving = self.state.moving().map(|(id, ..)| id);
        let mut sprites = HashMap::new();
        for annotation in &annotations {
            // A dragged annotation keeps the tile it had; it is rebuilt
            // where it is dropped.
            if moving == Some(annotation.id())
                && let Some(sprite) = self.sprites.remove(&annotation.id())
            {
                sprites.insert(annotation.id(), sprite);
                continue;
            }
            let Some(tile) = self.sprite_tiles.get(annotation, frame.as_ref()) else {
                continue;
            };
            let sprite = match self.sprites.remove(&annotation.id()) {
                Some(sprite) if Arc::ptr_eq(sprite.tile.image(), tile.image()) => sprite,
                Some(stale) => {
                    self.stale_images.push(stale.image);
                    TileSprite::new(tile)
                }
                None => TileSprite::new(tile),
            };
            sprites.insert(annotation.id(), sprite);
        }
        self.stale_images
            .extend(self.sprites.drain().map(|(_, sprite)| sprite.image));
        self.sprites = sprites;
        self.refresh_draft(frame.as_ref());
    }

    /// Rasterizes the annotation being drawn.
    ///
    /// A freehand stroke is cut into chunks of [`DRAFT_CHUNK_POINTS`]: a
    /// finished chunk keeps its tile, so each move rasterizes only the end
    /// of the stroke, however long it grows. A rectangle needs no tile; the
    /// overlay draws it as a bordered quad.
    fn refresh_draft(&mut self, frame: Option<&Frame>) {
        if let Some(stale) = self.draft_sprite.take() {
            self.stale_images.push(stale.image);
        }
        let Some(draft) = self.state.draft().cloned() else {
            let chunks = std::mem::take(&mut self.draft_chunks);
            self.stale_images
                .extend(chunks.sprites.into_iter().map(|sprite| sprite.image));
            return;
        };
        if self.draft_chunks.id != Some(draft.id()) {
            let chunks = std::mem::replace(
                &mut self.draft_chunks,
                DraftChunks {
                    id: Some(draft.id()),
                    ..DraftChunks::default()
                },
            );
            self.stale_images
                .extend(chunks.sprites.into_iter().map(|sprite| sprite.image));
        }
        let tail = match draft.shape() {
            Shape::Rectangle { .. } => return,
            Shape::Pen { points } | Shape::Marker { points } | Shape::Mosaic { points } => {
                let points = points.clone();
                let with_points = |slice: &[ScenePoint]| {
                    let points: Arc<[ScenePoint]> = slice.into();
                    draft.with_shape(match draft.shape() {
                        Shape::Pen { .. } => Shape::Pen { points },
                        Shape::Marker { .. } => Shape::Marker { points },
                        _ => Shape::Mosaic { points },
                    })
                };
                // Chunks share their end point, so the stroke has no gaps.
                while points.len() - self.draft_chunks.start > DRAFT_CHUNK_POINTS {
                    let end = self.draft_chunks.start + DRAFT_CHUNK_POINTS;
                    let chunk = with_points(&points[self.draft_chunks.start..=end]);
                    if let Some(tile) = crate::raster::sprite(&chunk, frame) {
                        self.draft_chunks.sprites.push(TileSprite::new(tile));
                    }
                    self.draft_chunks.start = end;
                }
                with_points(&points[self.draft_chunks.start..])
            }
            _ => draft,
        };
        self.draft_sprite = crate::raster::sprite(&tail, frame).map(TileSprite::new);
    }

    /// Runs `change` on the state and redraws every overlay.
    pub(crate) fn update_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut SessionState, bool) -> Effect,
    ) {
        if self.is_finished {
            return;
        }
        let update_started = std::time::Instant::now();
        let scene = self.state.history().current().clone();
        let had_draft = self.state.draft().is_some();
        let effect = change(&mut self.state, self.is_detecting_windows);
        // Most changes are the pointer moving; tiles only follow the scene
        // and the draft.
        if had_draft
            || self.state.draft().is_some()
            || !self.state.history().current().is_same(&scene)
        {
            self.refresh_sprites();
        }
        let update_time = update_started.elapsed();
        if update_time.as_millis() >= 4 {
            tracing::debug!("session update took {update_time:?}");
        }
        cx.notify();
        self.apply(effect, window, cx);
    }

    fn apply(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match effect {
            Effect::None => {}
            Effect::Finish(outcome) => self.finish(outcome, cx),
            Effect::EditText => self.begin_text(window, cx),
            Effect::CommitText => self.commit_text(window, cx),
        }
    }

    fn begin_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("text editing begins at {:?}", self.state.text_draft());
        let input = cx.new(|cx| InputState::new(window, cx));
        self._subscriptions.push(
            cx.subscribe_in(&input, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.commit_text(window, cx);
                }
            }),
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.text_input = Some(input);
    }

    /// Ends text editing with what the field holds.
    pub(crate) fn commit_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.text_input.take() else {
            return;
        };
        let content = input.read(cx).value();
        tracing::debug!("text editing ends with {content:?}");
        self.state.commit_text(&content);
        self.refresh_sprites();
        cx.notify();
        cx.emit(SessionEvent::TextEnded);
    }

    /// Ends the session: closes the overlays and hands the result on.
    pub(crate) fn finish(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        tracing::debug!("session ends: {outcome:?}");
        if self.is_finished {
            return;
        }
        self.is_finished = true;
        app::remember_style(self.state.style(), cx);

        let delivery = self.state.selection().and_then(|selection| {
            let frame = self.capture.frame_at(selection.origin())?.clone();
            Some(output::Delivery::new(
                frame,
                selection,
                self.state.history().current().clone(),
                std::mem::take(&mut self.tiles),
            ))
        });
        // Overlays first, so the frozen screen never shows without them.
        let windows: Vec<AnyWindowHandle> = self
            .overlays
            .drain(..)
            .map(|overlay| overlay.handle)
            .chain(self.backdrops.drain(..))
            .collect();
        // The overlay asking to finish is mid-update; close windows after it.
        cx.defer(move |cx| {
            for window in windows {
                window
                    .update(cx, |_, window, _| window.remove_window())
                    .ok();
            }
            app::end_session(cx);
            if let Some(delivery) = delivery
                && outcome != Outcome::Cancel
            {
                output::deliver(outcome, delivery, cx);
            }
        });
    }

    /// Copies the color under the pointer.
    pub(crate) fn copy_color(&mut self, notation: ColorNotation, cx: &mut Context<Self>) {
        let Some(color) = self.color_at_pointer() else {
            return;
        };
        let text = notation.format(color);
        let clipboard = output::clipboard::clipboard(cx);
        cx.background_spawn(async move {
            if let Err(error) = clipboard.write_text(&text) {
                tracing::warn!("{error:#}");
            }
        })
        .detach();
        self.finish(Outcome::Cancel, cx);
    }

    pub(crate) fn color_at_pointer(&self) -> Option<Color> {
        let pointer = machine::physical(self.state.pointer()?);
        self.capture.frame_at(pointer)?.pixel(pointer)
    }

    pub(crate) fn set_tool(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
        if self.text_input.is_some() {
            self.commit_text(window, cx);
        }
        self.update_state(window, cx, |state, _| {
            state.toggle_tool(tool);
            Effect::None
        });
    }
}

/// Events overlays react to.
pub(crate) enum SessionEvent {
    /// Text editing ended; the overlay takes focus back from the field.
    TextEnded,
}

impl gpui_kit::EventEmitter<SessionEvent> for CaptureSession {}

/// Freezes the screen and opens the overlays. Ignored while a session is
/// already open or being opened.
pub fn start(cx: &mut App) {
    if !app::is_idle(cx) {
        return;
    }
    let capturer = app::capturer(cx);
    let detect_windows = app::settings(cx).is_detecting_windows();
    let task = cx.spawn(async move |cx| {
        let captured = cx
            .background_spawn(async move {
                let started = std::time::Instant::now();
                let capture = crate::capture::capture_all(&*capturer, detect_windows)?;
                tracing::info!(
                    "captured {} displays and {} windows in {:?}",
                    capture.frames().len(),
                    capture.windows().len(),
                    started.elapsed()
                );
                let images = render_images(&capture);
                anyhow::Ok((capture, images))
            })
            .await;
        cx.update(|cx| {
            app::capture_finished(cx);
            match captured {
                Ok((capture, images)) => {
                    if let Err(error) = open_with(capture, images, cx) {
                        tracing::error!("cannot show the capture: {error:#}");
                        app::end_session(cx);
                    }
                }
                Err(error) => {
                    tracing::error!("cannot capture the screen: {error:#}");
                    crate::shell::hud::show(format!("Couldn’t capture the screen: {error}"), cx);
                }
            }
        });
    });
    app::capture_started(task, cx);
}

/// The frozen frames as GPUI images, by frame index.
pub(crate) fn render_images(capture: &CaptureSet) -> Vec<Arc<RenderImage>> {
    capture
        .frames()
        .iter()
        .map(|frame| {
            let (width, height) = (frame.bounds().width as u32, frame.bounds().height as u32);
            let row_bytes = width as usize * 4;
            // One pass from the frame's RGBA to GPUI's BGRA.
            let bgra = crate::capture::convert_rows(
                frame.pixels(),
                height as usize,
                row_bytes,
                row_bytes,
                |row, out| {
                    for (pixel, out) in row[..row_bytes]
                        .chunks_exact(4)
                        .zip(out.chunks_exact_mut(4))
                    {
                        out.copy_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                    }
                },
            );
            let image =
                RgbaImage::from_raw(width, height, bgra).expect("a frame's pixels match its size");
            Arc::new(RenderImage::new(smallvec![image::Frame::new(image)]))
        })
        .collect()
}

/// The overlay windows of the open session, the focused one last.
#[cfg_attr(not(any(test, feature = "preview")), allow(dead_code))]
pub(crate) fn overlay_windows(cx: &App) -> Vec<AnyWindowHandle> {
    app::session(cx).map_or_else(Vec::new, |session| {
        session
            .read(cx)
            .overlays
            .iter()
            .map(|overlay| overlay.handle)
            .collect()
    })
}

/// Opens a session on `capture` with one overlay per display.
pub(crate) fn open_with(
    capture: CaptureSet,
    images: Vec<Arc<RenderImage>>,
    cx: &mut App,
) -> anyhow::Result<()> {
    let capture = fit_lone_frame(capture, cx);
    let pointer = capture.pointer();
    let session = cx.new(|cx| CaptureSession::new(capture.clone(), images, cx));
    app::begin_session(session.clone(), cx);

    let displays = cx.displays();
    let is_visible = !app::is_offscreen(cx);
    // On macOS the frozen frame sits in a window of its own that is drawn
    // once, under a transparent overlay that alone redraws. On Windows that
    // costs more than it saves: the compositor blends the display-sized
    // transparent overlay over the frame on every redraw, which saturates
    // integrated graphics on a 4K HDR desktop, while an opaque overlay
    // drawing the frame itself takes the compositor's cheapest path (see
    // `main`). X11 window managers don't reliably keep two full-screen
    // windows stacked, and a preview renders one window, so those draw the
    // frame in the overlay too.
    let has_backdrops = is_visible && cfg!(target_os = "macos");
    session.update(cx, |session, _| session.has_backdrops = has_backdrops);
    let mut backdrops = Vec::new();
    let frames = capture.frames();
    let focused_ix = pointer
        .and_then(|pointer| {
            frames
                .iter()
                .position(|frame| frame.bounds().contains(pointer))
        })
        .unwrap_or(0);
    let mut overlays = Vec::new();
    // The display under the pointer opens last, so it ends up in front.
    let mut order: Vec<usize> = (0..frames.len()).filter(|ix| *ix != focused_ix).collect();
    order.push(focused_ix);
    for frame_ix in order {
        let frame = &frames[frame_ix];
        let display = displays
            .iter()
            .find(|display| matches_display(frame, display.id(), display.bounds()));
        let Some(display) = display else {
            tracing::warn!(
                "no display matches a captured frame at {:?}",
                frame.bounds()
            );
            continue;
        };
        if has_backdrops {
            let image = session.read(cx).images[frame_ix].clone();
            let options = overlay_options(display.id(), display.bounds(), false);
            let (handle, _) = gpui_kit::open_window(options, cx, move |_, cx| {
                cx.new(|_| backdrop::Backdrop::new(image))
            })?;
            handle
                .update(cx, |_, window, _| {
                    crate::shell::platform::ensure_shown(window, false)
                })
                .ok();
            // The backdrop never redraws on its own, and its first frame
            // can be drawn while the window is still hidden; draw it again
            // once it is on screen.
            cx.spawn(async move |cx| {
                for delay in [16, 100, 300] {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(delay))
                        .await;
                    if handle.update(cx, |_, window, _| window.refresh()).is_err() {
                        break;
                    }
                }
            })
            .detach();
            backdrops.push(handle);
        }
        let mut options = overlay_options(display.id(), display.bounds(), frame_ix == focused_ix);
        options.show = is_visible;
        options.focus &= is_visible;
        if has_backdrops {
            options.window_background = gpui_kit::WindowBackgroundAppearance::Transparent;
        }
        let session = session.clone();
        let display_size = display.bounds().size;
        let build = move |window: &mut Window, cx: &mut App| {
            // A hidden window is never placed, so it keeps a default size.
            if !is_visible {
                window.resize(display_size);
            }
            cx.new(|cx| overlay::Overlay::new(session, frame_ix, window, cx))
        };
        let (handle, view) = if has_backdrops {
            // Over a backdrop the overlay must be transparent where it draws
            // nothing, and GPUI Kit's Root paints the theme background over
            // the whole window. The overlay is opened as its own root
            // instead, and sets the type and rem size Root would.
            let mut built = None;
            let handle = cx.open_window(options, |window, cx| {
                let view = build(window, cx);
                built = Some(view.clone());
                view
            })?;
            (handle.into(), built.expect("the overlay was built"))
        } else {
            gpui_kit::open_window(options, cx, build)?
        };
        let is_focused = frame_ix == focused_ix;
        if is_visible {
            handle
                .update(cx, |_, window, _| {
                    crate::shell::platform::ensure_shown(window, is_focused)
                })
                .ok();
        }
        if is_focused {
            handle
                .update(cx, |_, window, cx| {
                    if is_visible {
                        window.activate_window();
                    }
                    let focus = view.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                })
                .ok();
        }
        tracing::debug!("opened an overlay on {:?}", frame.bounds());
        overlays.push(OverlayWindow { handle });
    }
    if overlays.is_empty() {
        anyhow::bail!("no display could show the capture");
    }
    session.update(cx, |session, _| {
        session.overlays = overlays;
        session.backdrops = backdrops;
    });
    Ok(())
}

/// Gives a lone frame that matches no display the scale of the lone
/// display. The Wayland screenshot portal hands out device pixels without
/// saying how they relate to the desktop's logical layout; with one display
/// and one image, the ratio of their widths is that scale.
fn fit_lone_frame(capture: CaptureSet, cx: &App) -> CaptureSet {
    let displays = cx.displays();
    let ([frame], [display]) = (capture.frames(), displays.as_slice()) else {
        return capture;
    };
    if matches_display(frame, display.id(), display.bounds()) {
        return capture;
    }
    let logical_width = f32::from(display.bounds().size.width);
    if logical_width <= 0. {
        return capture;
    }
    let scale = frame.bounds().width as f32 / logical_width;
    let frame = frame.clone().with_scale(scale);
    capture.with_frames(vec![frame])
}

/// Whether the GPUI display `id` with logical `bounds` is the one `frame`
/// was captured from.
fn matches_display(frame: &Frame, id: DisplayId, bounds: Bounds<Pixels>) -> bool {
    matches_area(&frame.area(), frame.native_id(), id, bounds)
}

/// Whether the GPUI display `id` with logical `bounds` is the captured
/// display at `area`: by identifier where the platform shares GPUI's (an
/// `HMONITOR` on Windows), else by position and size.
pub(crate) fn matches_area(
    area: &DisplayArea,
    native_id: u64,
    id: DisplayId,
    bounds: Bounds<Pixels>,
) -> bool {
    if native_id != 0 && u64::from(id) == native_id {
        return true;
    }
    let logical = |value: i32| value as f32 / area.scale();
    let near = |a: f32, b: Pixels| (a - f32::from(b)).abs() <= 2.;
    let physical = area.bounds();
    near(logical(physical.width), bounds.size.width)
        && near(logical(physical.height), bounds.size.height)
        && (near(logical(physical.x), bounds.origin.x) || near(physical.x as f32, bounds.origin.x))
        && (near(logical(physical.y), bounds.origin.y) || near(physical.y as f32, bounds.origin.y))
}

/// A borderless window covering one display, above everything else.
///
/// macOS and Windows get a pop-up: a panel that joins every space on macOS,
/// a topmost tool window on Windows. On X11 a pop-up is override-redirect
/// and never receives keyboard focus, so Linux gets a full-screen normal
/// window instead.
fn overlay_options(display: DisplayId, bounds: Bounds<Pixels>, focus: bool) -> WindowOptions {
    let is_linux = cfg!(target_os = "linux");
    WindowOptions {
        window_bounds: Some(if is_linux {
            WindowBounds::Fullscreen(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }),
        titlebar: None,
        focus,
        show: true,
        kind: if is_linux {
            WindowKind::Normal
        } else {
            WindowKind::PopUp
        },
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display),
        app_id: Some("snip".into()),
        window_decorations: is_linux.then_some(gpui_kit::WindowDecorations::Client),
        ..Default::default()
    }
}

/// Logical window coordinates of a desktop rectangle on `area`.
pub(crate) fn logical_bounds(area: &DisplayArea, rect: PhysRect) -> Bounds<Pixels> {
    let (x, y) = area.to_logical(rect.x as f32, rect.y as f32);
    let scale = area.scale();
    Bounds::new(
        point(px(x), px(y)),
        size(
            px(rect.width as f32 / scale),
            px(rect.height as f32 / scale),
        ),
    )
}

/// The desktop point under a logical window position on `area`.
pub(crate) fn scene_point(area: &DisplayArea, position: Point<Pixels>) -> ScenePoint {
    let (x, y) = area.to_physical(position.x.into(), position.y.into());
    ScenePoint::new(x, y)
}

pub(crate) fn logical_point(area: &DisplayArea, point_: ScenePoint) -> Point<Pixels> {
    let (x, y) = area.to_logical(point_.x, point_.y);
    point(px(x), px(y))
}

#[cfg(test)]
mod tests;
