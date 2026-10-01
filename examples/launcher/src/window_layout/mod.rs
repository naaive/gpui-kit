//! Window Management: moves and resizes the window that was in front before
//! the launcher, into halves, thirds, quarters, the center, or onto another
//! display.
//!
//! The frame arithmetic is plain Rust; the platform part only reads the
//! frontmost window, its display's usable area, and sets the frame. Windows
//! is supported; elsewhere the commands are not offered.

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
}

impl Layout {
    pub const ALL: [Self; 23] = [
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
        }
    }

    /// Where the window goes on a display whose usable area is `area`, given
    /// its current frame. `None` for layouts that are not a frame on the
    /// current display.
    pub fn frame(self, window: Frame, area: Frame) -> Option<Frame> {
        Some(match self {
            Self::LeftHalf => area.columns(0., 0.5),
            Self::RightHalf => area.columns(0.5, 1.),
            Self::TopHalf => area.rows(0., 0.5),
            Self::BottomHalf => area.rows(0.5, 1.),
            Self::TopLeftQuarter => area.columns(0., 0.5).rows(0., 0.5),
            Self::TopRightQuarter => area.columns(0.5, 1.).rows(0., 0.5),
            Self::BottomLeftQuarter => area.columns(0., 0.5).rows(0.5, 1.),
            Self::BottomRightQuarter => area.columns(0.5, 1.).rows(0.5, 1.),
            Self::FirstThird => area.columns(0., 1. / 3.),
            Self::CenterThird => area.columns(1. / 3., 2. / 3.),
            Self::LastThird => area.columns(2. / 3., 1.),
            Self::FirstTwoThirds => area.columns(0., 2. / 3.),
            Self::LastTwoThirds => area.columns(1. / 3., 1.),
            Self::Maximize => area,
            Self::AlmostMaximize => area.centered(
                (area.width as f32 * 0.9) as i32,
                (area.height as f32 * 0.9) as i32,
            ),
            Self::MaximizeHeight => Frame::new(window.x, area.y, window.width, area.height),
            Self::MaximizeWidth => Frame::new(area.x, window.y, area.width, window.height),
            Self::ReasonableSize => area.centered(
                (area.width as f32 * 0.6) as i32,
                (area.height as f32 * 0.7) as i32,
            ),
            Self::Center => area.centered(window.width, window.height),
            Self::Restore | Self::NextDisplay | Self::PreviousDisplay | Self::Minimize => {
                return None;
            }
        })
    }

    fn item(self) -> Item {
        Item::new(ItemId::new(format!("window/{}", self.id())), self.title())
            .with_subtitle("Window Management")
            .with_icon(self.icon())
            .with_accessory(Accessory::text("Command"))
            .with_keyword("window")
            .with_keyword("snap")
            .with_keyword("resize")
            .with_action(Action::new(
                self.title(),
                Effect::Run(RunHandler::new(move |(), _, cx| run(self, cx))),
            ))
    }
}

/// The Window Management commands this platform supports.
pub fn commands() -> Vec<Item> {
    match is_supported() {
        true => Layout::ALL.into_iter().map(Layout::item).collect(),
        false => Vec::new(),
    }
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
    crate::shell::launcher::hide(cx);
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        // The launcher's window has to be gone first, or it is the one
        // the system brings back to the front.
        executor.timer(std::time::Duration::from_millis(120)).await;
        if let Err(message) = apply(layout) {
            cx.update(|cx| crate::shell::platform::show_hud(message, cx));
        }
    })
    .detach();
}

fn apply(layout: Layout) -> Result<(), SharedString> {
    #[cfg(target_os = "windows")]
    {
        windows::apply(layout)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = layout;
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
        let frame = |layout: Layout| layout.frame(window, AREA).unwrap();
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
            Layout::Center.frame(window, AREA),
            Some(Frame::new(500, 300, 400, 300))
        );
        let huge = Frame::new(0, 0, 4000, 3000);
        assert_eq!(Layout::Center.frame(huge, AREA), Some(AREA));
        assert_eq!(
            Layout::MaximizeHeight.frame(window, AREA),
            Some(Frame::new(0, 0, 400, 900))
        );
        assert_eq!(Layout::Restore.frame(window, AREA), None);
    }
}
