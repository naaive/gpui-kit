//! Window Management: moves and resizes the window that was in front before
//! the launcher, into halves, thirds, quarters, the center, or onto another
//! display.
//!
//! The frame arithmetic is plain Rust; the platform part only reads the
//! frontmost window, its display's usable area, and sets the frame. Windows
//! is supported; elsewhere the commands are not offered.

mod custom;
#[cfg(target_os = "windows")]
mod windows;

use gpui_kit::{App, SharedString};

use crate::model::{Accessory, Action, Effect, Item, ItemId, RunHandler};

/// A rectangle in physical pixels: the visible frame of a window, or a
/// display's usable area (without the taskbar or dock).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Frame {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The part of `self` from `start` to `end`, as fractions of its width.
    fn columns(self, start: f32, end: f32) -> Self {
        let left = self.x + (self.width as f32 * start).round() as i32;
        let right = self.x + (self.width as f32 * end).round() as i32;
        Self::new(left, self.y, right - left, self.height)
    }

    /// The part of `self` from `start` to `end`, as fractions of its height.
    fn rows(self, start: f32, end: f32) -> Self {
        let top = self.y + (self.height as f32 * start).round() as i32;
        let bottom = self.y + (self.height as f32 * end).round() as i32;
        Self::new(self.x, top, self.width, bottom - top)
    }

    /// `self` with `amount` pixels taken off every side.
    fn inset(self, amount: i32) -> Self {
        let amount = amount.max(0).min(self.width / 4).min(self.height / 4);
        Self::new(
            self.x + amount,
            self.y + amount,
            self.width - 2 * amount,
            self.height - 2 * amount,
        )
    }

    /// Whether `other` is `self`, give or take a few pixels.
    fn is_near(self, other: Self) -> bool {
        (self.x - other.x).abs() <= SAME_FRAME
            && (self.y - other.y).abs() <= SAME_FRAME
            && (self.width - other.width).abs() <= SAME_FRAME
            && (self.height - other.height).abs() <= SAME_FRAME
    }

    /// A `width` × `height` frame centered in `self`, no larger than it.
    fn centered(self, width: i32, height: i32) -> Self {
        let (width, height) = (width.min(self.width), height.min(self.height));
        Self::new(
            self.x + (self.width - width) / 2,
            self.y + (self.height - height) / 2,
            width,
            height,
        )
    }
}

/// A place to put the window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeftQuarter,
    TopRightQuarter,
    BottomLeftQuarter,
    BottomRightQuarter,
    FirstThird,
    CenterThird,
    LastThird,
    FirstTwoThirds,
    LastTwoThirds,
    Maximize,
    AlmostMaximize,
    MaximizeHeight,
    MaximizeWidth,
    ReasonableSize,
    Center,
    Restore,
    NextDisplay,
    PreviousDisplay,
    Minimize,
    FirstFourth,
    SecondFourth,
    ThirdFourth,
    LastFourth,
    TopLeftSixth,
    TopCenterSixth,
    TopRightSixth,
    BottomLeftSixth,
    BottomCenterSixth,
    BottomRightSixth,
    CenterHalf,
    CenterTwoThirds,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MakeLarger,
    MakeSmaller,
    /// A user's own layout.
    Custom(CustomFrame),
}

/// Where a custom layout puts the window, in thousandths of the display's
/// usable area.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CustomFrame {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// How much Make Larger and Make Smaller change each side, as a fraction of
/// the usable area.
const RESIZE_STEP: f32 = 0.05;
/// Frames this close are the same frame, for cycling: windows round their
/// sizes to their own increments.
const SAME_FRAME: i32 = 12;

impl Layout {
    pub const ALL: [Self; 41] = [
        Self::LeftHalf,
        Self::RightHalf,
        Self::TopHalf,
        Self::BottomHalf,
        Self::TopLeftQuarter,
        Self::TopRightQuarter,
        Self::BottomLeftQuarter,
        Self::BottomRightQuarter,
        Self::FirstThird,
        Self::CenterThird,
        Self::LastThird,
        Self::FirstTwoThirds,
        Self::LastTwoThirds,
        Self::Maximize,
        Self::AlmostMaximize,
        Self::MaximizeHeight,
        Self::MaximizeWidth,
        Self::ReasonableSize,
        Self::Center,
        Self::Restore,
        Self::NextDisplay,
        Self::PreviousDisplay,
        Self::Minimize,
        Self::FirstFourth,
        Self::SecondFourth,
        Self::ThirdFourth,
        Self::LastFourth,
        Self::TopLeftSixth,
        Self::TopCenterSixth,
        Self::TopRightSixth,
        Self::BottomLeftSixth,
        Self::BottomCenterSixth,
        Self::BottomRightSixth,
        Self::CenterHalf,
        Self::CenterTwoThirds,
        Self::MoveLeft,
        Self::MoveRight,
        Self::MoveUp,
        Self::MoveDown,
        Self::MakeLarger,
        Self::MakeSmaller,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::LeftHalf => "left-half",
            Self::RightHalf => "right-half",
            Self::TopHalf => "top-half",
            Self::BottomHalf => "bottom-half",
            Self::TopLeftQuarter => "top-left-quarter",
            Self::TopRightQuarter => "top-right-quarter",
            Self::BottomLeftQuarter => "bottom-left-quarter",
            Self::BottomRightQuarter => "bottom-right-quarter",
            Self::FirstThird => "first-third",
            Self::CenterThird => "center-third",
            Self::LastThird => "last-third",
            Self::FirstTwoThirds => "first-two-thirds",
            Self::LastTwoThirds => "last-two-thirds",
            Self::Maximize => "maximize",
            Self::AlmostMaximize => "almost-maximize",
            Self::MaximizeHeight => "maximize-height",
            Self::MaximizeWidth => "maximize-width",
            Self::ReasonableSize => "reasonable-size",
            Self::Center => "center",
            Self::Restore => "restore",
            Self::NextDisplay => "next-display",
            Self::PreviousDisplay => "previous-display",
            Self::Minimize => "minimize",
            Self::FirstFourth => "first-fourth",
            Self::SecondFourth => "second-fourth",
            Self::ThirdFourth => "third-fourth",
            Self::LastFourth => "last-fourth",
            Self::TopLeftSixth => "top-left-sixth",
            Self::TopCenterSixth => "top-center-sixth",
            Self::TopRightSixth => "top-right-sixth",
            Self::BottomLeftSixth => "bottom-left-sixth",
            Self::BottomCenterSixth => "bottom-center-sixth",
            Self::BottomRightSixth => "bottom-right-sixth",
            Self::CenterHalf => "center-half",
            Self::CenterTwoThirds => "center-two-thirds",
            Self::MoveLeft => "move-left",
            Self::MoveRight => "move-right",
            Self::MoveUp => "move-up",
            Self::MoveDown => "move-down",
            Self::MakeLarger => "make-larger",
            Self::MakeSmaller => "make-smaller",
            Self::Custom(_) => "custom",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::LeftHalf => "Left Half",
            Self::RightHalf => "Right Half",
            Self::TopHalf => "Top Half",
            Self::BottomHalf => "Bottom Half",
            Self::TopLeftQuarter => "Top Left Quarter",
            Self::TopRightQuarter => "Top Right Quarter",
            Self::BottomLeftQuarter => "Bottom Left Quarter",
            Self::BottomRightQuarter => "Bottom Right Quarter",
            Self::FirstThird => "First Third",
            Self::CenterThird => "Center Third",
            Self::LastThird => "Last Third",
            Self::FirstTwoThirds => "First Two Thirds",
            Self::LastTwoThirds => "Last Two Thirds",
            Self::Maximize => "Maximize",
            Self::AlmostMaximize => "Almost Maximize",
            Self::MaximizeHeight => "Maximize Height",
            Self::MaximizeWidth => "Maximize Width",
            Self::ReasonableSize => "Reasonable Size",
            Self::Center => "Center",
            Self::Restore => "Restore",
            Self::NextDisplay => "Next Display",
            Self::PreviousDisplay => "Previous Display",
            Self::Minimize => "Minimize",
            Self::FirstFourth => "First Fourth",
            Self::SecondFourth => "Second Fourth",
            Self::ThirdFourth => "Third Fourth",
            Self::LastFourth => "Last Fourth",
            Self::TopLeftSixth => "Top Left Sixth",
            Self::TopCenterSixth => "Top Center Sixth",
            Self::TopRightSixth => "Top Right Sixth",
            Self::BottomLeftSixth => "Bottom Left Sixth",
            Self::BottomCenterSixth => "Bottom Center Sixth",
            Self::BottomRightSixth => "Bottom Right Sixth",
            Self::CenterHalf => "Center Half",
            Self::CenterTwoThirds => "Center Two Thirds",
            Self::MoveLeft => "Move Left",
            Self::MoveRight => "Move Right",
            Self::MoveUp => "Move Up",
            Self::MoveDown => "Move Down",
            Self::MakeLarger => "Make Larger",
            Self::MakeSmaller => "Make Smaller",
            Self::Custom(_) => "Custom Layout",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::LeftHalf | Self::FirstThird | Self::FirstTwoThirds => "panel-left",
            Self::RightHalf | Self::LastThird | Self::LastTwoThirds => "panel-right",
            Self::TopHalf => "panel-top",
            Self::BottomHalf => "panel-bottom",
            Self::TopLeftQuarter
            | Self::TopRightQuarter
            | Self::BottomLeftQuarter
            | Self::BottomRightQuarter => "layout-grid",
            Self::CenterThird | Self::Center | Self::ReasonableSize => "app-window",
            Self::Maximize | Self::AlmostMaximize => "maximize",
            Self::MaximizeHeight | Self::MaximizeWidth => "move",
            Self::Restore => "undo-2",
            Self::NextDisplay => "monitor",
            Self::PreviousDisplay => "monitor",
            Self::Minimize => "minimize",
            Self::FirstFourth | Self::SecondFourth | Self::ThirdFourth | Self::LastFourth => {
                "columns-4"
            }
            Self::TopLeftSixth
            | Self::TopCenterSixth
            | Self::TopRightSixth
            | Self::BottomLeftSixth
            | Self::BottomCenterSixth
            | Self::BottomRightSixth => "grid-3x2",
            Self::CenterHalf | Self::CenterTwoThirds => "columns-3",
            Self::MoveLeft => "arrow-left-to-line",
            Self::MoveRight => "arrow-right-to-line",
            Self::MoveUp => "arrow-up-to-line",
            Self::MoveDown => "arrow-down-to-line",
            Self::MakeLarger => "maximize-2",
            Self::MakeSmaller => "minimize-2",
            Self::Custom(_) => "layout-template",
        }
    }

    /// Where the window goes on a display whose usable area is `area`, given
    /// its current frame, with `gap` pixels between windows and around the
    /// edges. `None` for layouts that are not a frame on the current display.
    ///
    /// The halves cycle as in Raycast: asking for the half the window
    /// already fills gives two thirds, then one third, then the half again.
    pub fn frame(self, window: Frame, area: Frame, gap: i32) -> Option<Frame> {
        // Tiles share the gap: half of it around each tile, inside an area
        // inset by the other half, makes `gap` everywhere.
        let half_gap = gap / 2;
        let tiles = area.inset(gap - half_gap);
        let tile = |frame: Frame| frame.inset(half_gap);
        let cycle = |sizes: [Frame; 3]| {
            let at = sizes
                .iter()
                .position(|size| tile(*size).is_near(window))
                .map_or(0, |index| (index + 1) % sizes.len());
            tile(sizes[at])
        };
        let inner = area.inset(gap);
        Some(match self {
            Self::LeftHalf => cycle([
                tiles.columns(0., 0.5),
                tiles.columns(0., 2. / 3.),
                tiles.columns(0., 1. / 3.),
            ]),
            Self::RightHalf => cycle([
                tiles.columns(0.5, 1.),
                tiles.columns(1. / 3., 1.),
                tiles.columns(2. / 3., 1.),
            ]),
            Self::TopHalf => cycle([
                tiles.rows(0., 0.5),
                tiles.rows(0., 2. / 3.),
                tiles.rows(0., 1. / 3.),
            ]),
            Self::BottomHalf => cycle([
                tiles.rows(0.5, 1.),
                tiles.rows(1. / 3., 1.),
                tiles.rows(2. / 3., 1.),
            ]),
            Self::TopLeftQuarter => tile(tiles.columns(0., 0.5).rows(0., 0.5)),
            Self::TopRightQuarter => tile(tiles.columns(0.5, 1.).rows(0., 0.5)),
            Self::BottomLeftQuarter => tile(tiles.columns(0., 0.5).rows(0.5, 1.)),
            Self::BottomRightQuarter => tile(tiles.columns(0.5, 1.).rows(0.5, 1.)),
            Self::FirstThird => tile(tiles.columns(0., 1. / 3.)),
            Self::CenterThird => tile(tiles.columns(1. / 3., 2. / 3.)),
            Self::LastThird => tile(tiles.columns(2. / 3., 1.)),
            Self::FirstTwoThirds => tile(tiles.columns(0., 2. / 3.)),
            Self::LastTwoThirds => tile(tiles.columns(1. / 3., 1.)),
            Self::FirstFourth => tile(tiles.columns(0., 0.25)),
            Self::SecondFourth => tile(tiles.columns(0.25, 0.5)),
            Self::ThirdFourth => tile(tiles.columns(0.5, 0.75)),
            Self::LastFourth => tile(tiles.columns(0.75, 1.)),
            Self::TopLeftSixth => tile(tiles.columns(0., 1. / 3.).rows(0., 0.5)),
            Self::TopCenterSixth => tile(tiles.columns(1. / 3., 2. / 3.).rows(0., 0.5)),
            Self::TopRightSixth => tile(tiles.columns(2. / 3., 1.).rows(0., 0.5)),
            Self::BottomLeftSixth => tile(tiles.columns(0., 1. / 3.).rows(0.5, 1.)),
            Self::BottomCenterSixth => tile(tiles.columns(1. / 3., 2. / 3.).rows(0.5, 1.)),
            Self::BottomRightSixth => tile(tiles.columns(2. / 3., 1.).rows(0.5, 1.)),
            Self::CenterHalf => tile(tiles.columns(0.25, 0.75)),
            Self::CenterTwoThirds => tile(tiles.columns(1. / 6., 5. / 6.)),
            Self::Maximize => inner,
            Self::AlmostMaximize => area.centered(
                (area.width as f32 * 0.9) as i32,
                (area.height as f32 * 0.9) as i32,
            ),
            Self::MaximizeHeight => Frame::new(window.x, inner.y, window.width, inner.height),
            Self::MaximizeWidth => Frame::new(inner.x, window.y, inner.width, window.height),
            Self::ReasonableSize => area.centered(
                (area.width as f32 * 0.6) as i32,
                (area.height as f32 * 0.7) as i32,
            ),
            Self::Center => area.centered(window.width, window.height),
            Self::MoveLeft => Frame::new(
                inner.x,
                window.y,
                window.width.min(inner.width),
                window.height,
            ),
            Self::MoveRight => {
                let width = window.width.min(inner.width);
                Frame::new(
                    inner.x + inner.width - width,
                    window.y,
                    width,
                    window.height,
                )
            }
            Self::MoveUp => Frame::new(
                window.x,
                inner.y,
                window.width,
                window.height.min(inner.height),
            ),
            Self::MoveDown => {
                let height = window.height.min(inner.height);
                Frame::new(
                    window.x,
                    inner.y + inner.height - height,
                    window.width,
                    height,
                )
            }
            Self::MakeLarger | Self::MakeSmaller => {
                let sign = match self {
                    Self::MakeLarger => 1.,
                    _ => -1.,
                };
                let dx = (area.width as f32 * RESIZE_STEP * sign) as i32;
                let dy = (area.height as f32 * RESIZE_STEP * sign) as i32;
                let width = (window.width + 2 * dx).clamp(200.min(inner.width), inner.width);
                let height = (window.height + 2 * dy).clamp(150.min(inner.height), inner.height);
                let x = (window.x - (width - window.width) / 2)
                    .clamp(inner.x, inner.x + inner.width - width);
                let y = (window.y - (height - window.height) / 2)
                    .clamp(inner.y, inner.y + inner.height - height);
                Frame::new(x, y, width, height)
            }
            Self::Custom(custom) => {
                let part = |value: u16| f32::from(value.min(1000)) / 1000.;
                let left = part(custom.x);
                let top = part(custom.y);
                tile(
                    tiles
                        .columns(left, (left + part(custom.width)).min(1.))
                        .rows(top, (top + part(custom.height)).min(1.)),
                )
            }
            Self::Restore | Self::NextDisplay | Self::PreviousDisplay | Self::Minimize => {
                return None;
            }
        })
    }

    fn item(self) -> Item {
        self.item_with(format!("window/{}", self.id()), self.title().to_owned())
            .with_subtitle("Window Management")
    }

    fn item_with(self, id: String, title: String) -> Item {
        Item::new(ItemId::new(id), title.clone())
            .with_icon(self.icon())
            .with_accessory(Accessory::text("Command"))
            .with_keyword("window")
            .with_keyword("snap")
            .with_keyword("resize")
            .with_action(Action::new(
                title,
                Effect::Run(RunHandler::new(move |(), _, cx| run(self, cx))),
            ))
    }
}

/// The Window Management commands this platform supports.
pub fn commands() -> Vec<Item> {
    if !is_supported() {
        return Vec::new();
    }
    Layout::ALL
        .into_iter()
        .map(Layout::item)
        .chain(custom::load().iter().map(custom::item))
        .chain([
            Item::new(ItemId::new("window/create-layout"), "Create Window Layout")
                .with_subtitle("Window Management")
                .with_icon("layout-template")
                .with_accessory(Accessory::text("Command"))
                .with_keyword("custom")
                .with_keyword("window")
                .with_action(Action::new(
                    "Create Window Layout",
                    Effect::Push(crate::model::PushHandler::new(|window, cx| {
                        custom::layout_form(None, window, cx)
                    })),
                )),
        ])
        .collect()
}

fn is_supported() -> bool {
    cfg!(target_os = "windows")
}

/// Remembers the window in front, before the launcher takes its place, so
/// the commands know which window to move.
pub fn remember_frontmost() {
    #[cfg(target_os = "windows")]
    windows::remember_frontmost();
}

/// Hides the launcher, then lays out the window that was in front of it.
fn run(layout: Layout, cx: &mut App) {
    let gap = crate::shell::launcher::settings(cx).window_gap() as i32;
    crate::shell::launcher::hide(cx);
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        // The launcher's window has to be gone first, or it is the one
        // the system brings back to the front.
        executor.timer(std::time::Duration::from_millis(120)).await;
        if let Err(message) = apply(layout, gap) {
            cx.update(|cx| crate::shell::platform::show_hud(message, cx));
        }
    })
    .detach();
}

fn apply(layout: Layout, gap: i32) -> Result<(), SharedString> {
    #[cfg(target_os = "windows")]
    {
        windows::apply(layout, gap)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (layout, gap);
        Err("Window Management is not available here".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Frame = Frame {
        x: 100,
        y: 0,
        width: 1200,
        height: 900,
    };

    #[test]
    fn test_halves_thirds_and_quarters_tile_the_area() {
        let window = Frame::new(300, 200, 400, 300);
        let frame = |layout: Layout| layout.frame(window, AREA, 0).unwrap();
        assert_eq!(frame(Layout::LeftHalf), Frame::new(100, 0, 600, 900));
        assert_eq!(frame(Layout::RightHalf), Frame::new(700, 0, 600, 900));
        assert_eq!(frame(Layout::BottomHalf), Frame::new(100, 450, 1200, 450));
        assert_eq!(
            frame(Layout::BottomRightQuarter),
            Frame::new(700, 450, 600, 450)
        );
        assert_eq!(frame(Layout::CenterThird), Frame::new(500, 0, 400, 900));
        assert_eq!(frame(Layout::LastTwoThirds), Frame::new(500, 0, 800, 900));
        assert_eq!(frame(Layout::Maximize), AREA);
    }

    #[test]
    fn test_center_keeps_the_size_within_the_area() {
        let window = Frame::new(0, 0, 400, 300);
        assert_eq!(
            Layout::Center.frame(window, AREA, 0),
            Some(Frame::new(500, 300, 400, 300))
        );
        let huge = Frame::new(0, 0, 4000, 3000);
        assert_eq!(Layout::Center.frame(huge, AREA, 0), Some(AREA));
        assert_eq!(
            Layout::MaximizeHeight.frame(window, AREA, 0),
            Some(Frame::new(0, 0, 400, 900))
        );
        assert_eq!(Layout::Restore.frame(window, AREA, 0), None);
    }

    #[test]
    fn test_halves_cycle_and_gaps_separate_tiles() {
        let half = Layout::LeftHalf.frame(Frame::default(), AREA, 0).unwrap();
        assert_eq!(half, Frame::new(100, 0, 600, 900));
        let two_thirds = Layout::LeftHalf.frame(half, AREA, 0).unwrap();
        assert_eq!(two_thirds, Frame::new(100, 0, 800, 900));
        let third = Layout::LeftHalf.frame(two_thirds, AREA, 0).unwrap();
        assert_eq!(third, Frame::new(100, 0, 400, 900));
        assert_eq!(Layout::LeftHalf.frame(third, AREA, 0), Some(half));

        let left = Layout::LeftHalf.frame(Frame::default(), AREA, 8).unwrap();
        let right = Layout::RightHalf.frame(Frame::default(), AREA, 8).unwrap();
        assert_eq!(left.x, AREA.x + 8);
        assert_eq!(right.x - (left.x + left.width), 8);
        assert_eq!(right.x + right.width, AREA.x + AREA.width - 8);
        assert_eq!(Layout::Maximize.frame(left, AREA, 8).unwrap().y, 8);

        let custom = Layout::Custom(CustomFrame {
            x: 250,
            y: 0,
            width: 500,
            height: 1000,
        });
        assert_eq!(
            custom.frame(Frame::default(), AREA, 0),
            Some(Frame::new(400, 0, 600, 900))
        );
        let window = Frame::new(500, 300, 400, 300);
        let larger = Layout::MakeLarger.frame(window, AREA, 0).unwrap();
        assert_eq!(larger, Frame::new(440, 255, 520, 390));
        assert_eq!(
            Layout::MoveRight.frame(window, AREA, 0),
            Some(Frame::new(900, 300, 400, 300))
        );
    }
}
