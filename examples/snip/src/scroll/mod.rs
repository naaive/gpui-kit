//! Scrolling capture: the selected area, scrolled and captured again until
//! the end, joined into one tall image.
//!
//! The overlays close first, so the wheel reaches the application under the
//! area and the captures show it. Moving the pointer stops the capture.

mod input;
mod stitch;

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use image::RgbaImage;

use self::stitch::{Step, Stitcher};
use crate::{
    capture::Capturer,
    geometry::{PhysPoint, PhysRect},
};

/// Wheel notches per step: well under a screen, so captures overlap.
const NOTCHES: i32 = 3;
/// How long smooth scrolling gets to settle before the next capture.
const SETTLE: Duration = Duration::from_millis(350);
const MAX_STEPS: usize = 60;
/// The tallest image made, in pixels.
const MAX_HEIGHT: usize = 30_000;

/// Scrolls the content under `selection` and joins what it shows. Blocks
/// for as long as it scrolls; run it off the main thread.
pub fn capture(capturer: &dyn Capturer, selection: PhysRect) -> Result<RgbaImage> {
    if !input::is_supported() {
        bail!("scrolling capture isn't available on this platform");
    }
    // The overlays are closing; let the screen show what is under them.
    std::thread::sleep(Duration::from_millis(200));
    let center = PhysPoint::new(
        selection.x + selection.width / 2,
        selection.y + selection.height / 2,
    );
    input::move_pointer(center)?;
    let mut stitcher = Stitcher::new(capture_area(capturer, selection)?);
    for _ in 0..MAX_STEPS {
        input::scroll_down(NOTCHES)?;
        std::thread::sleep(SETTLE);
        if input::pointer().is_some_and(|pointer| pointer != center) {
            tracing::debug!("the pointer moved; scrolling capture stops");
            break;
        }
        match stitcher.push(capture_area(capturer, selection)?) {
            Step::Added(rows) => tracing::debug!("scrolled {rows} rows"),
            Step::Unchanged => break,
            Step::Lost => {
                tracing::debug!("the next capture doesn't continue the last");
                break;
            }
        }
        if stitcher.height() >= MAX_HEIGHT {
            break;
        }
    }
    Ok(stitcher.finish())
}

/// The pixels of `area` as the screen shows them now.
fn capture_area(capturer: &dyn Capturer, area: PhysRect) -> Result<RgbaImage> {
    capturer
        .capture_displays()?
        .iter()
        .find(|frame| frame.bounds().contains(area.origin()))
        .and_then(|frame| frame.crop(area))
        .context("the area is on no display")
}
