//! ScreenCaptureKit screenshots: one image of every display.
//!
//! ScreenCaptureKit answers on a dispatch queue of its own. Capture runs on
//! a background thread, which asks for every display at once and then waits
//! for the answers on a channel, up to a deadline; the main thread is never
//! blocked. Screenshots need macOS 14; earlier systems only offer streams.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail};
use block2::RcBlock;
use objc2::{AnyThread as _, rc::Retained, runtime::AnyClass, sel};
use objc2_core_graphics::{
    CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
};

use super::{Display, to_physical_rect};
use crate::{
    capture::{Frame, bgra_to_rgba},
    geometry::{DisplayArea, PhysRect},
};

/// How long to wait for ScreenCaptureKit, for the display list and then for
/// all screenshots together. Both are normally ready within a frame or two.
const TIMEOUT: Duration = Duration::from_secs(2);

/// `kCVPixelFormatType_32BGRA`: four bytes per pixel, blue first in memory.
const PIXEL_FORMAT_BGRA: u32 = u32::from_be_bytes(*b"BGRA");

/// Captures `displays` at `scale` pixels per point. A display that fails is
/// left out; only when every display fails is the capture an error.
pub fn capture(displays: &[Display], scale: f64) -> Result<Vec<Frame>> {
    let has_screenshots = AnyClass::get(c"SCScreenshotManager").is_some_and(|class| {
        class
            .metaclass()
            .responds_to(sel!(captureImageWithFilter:configuration:completionHandler:))
    });
    if !has_screenshots {
        bail!("Screen capture needs macOS 14 or later.");
    }
    let content = shareable_content()?;
    let shared_displays = unsafe { content.displays() }.to_vec();

    // Ask for every display before waiting for any, so the screenshots are
    // taken as close together as ScreenCaptureKit allows.
    let pending: Vec<_> = displays
        .iter()
        .filter_map(|display| {
            let native_id = display.native_id;
            let Some(shared) = shared_displays
                .iter()
                .find(|shared| unsafe { shared.displayID() } == native_id)
            else {
                tracing::debug!(
                    "ScreenCaptureKit doesn't list display {native_id}, perhaps a mirror"
                );
                return None;
            };
            let bounds = to_physical_rect(display.points, scale);
            Some((native_id, bounds, request(shared, bounds)))
        })
        .collect();

    let deadline = Instant::now() + TIMEOUT;
    let mut frames = Vec::new();
    let mut failure = None;
    for (native_id, bounds, answer) in pending {
        let frame = answer
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| anyhow!("the display gave no image in time"))
            .and_then(|pixels| pixels)
            .and_then(|pixels| {
                Frame::new(
                    DisplayArea::new(bounds, scale as f32),
                    native_id as u64,
                    pixels,
                )
            });
        match frame {
            Ok(frame) => frames.push(frame),
            Err(error) => {
                tracing::error!("cannot capture display {native_id}: {error:#}");
                failure = Some(error);
            }
        }
    }
    match failure {
        Some(error) if frames.is_empty() => Err(error),
        _ => Ok(frames),
    }
}

/// What ScreenCaptureKit may capture, which includes the displays.
fn shareable_content() -> Result<Retained<SCShareableContent>> {
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // The content is only borrowed for the call; retaining it keeps
            // it alive on the waiting thread, which ScreenCaptureKit allows.
            let answer = unsafe { Retained::retain(content) }.ok_or_else(|| describe(error));
            sender.send(answer).ok();
        },
    );
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    receiver
        .recv_timeout(TIMEOUT)
        .context("ScreenCaptureKit listed no displays in time")?
        .map_err(|reason| anyhow!("ScreenCaptureKit cannot list the displays: {reason}"))
}

/// Asks for a screenshot of `display` at exactly `bounds`' pixel size, and
/// returns where its RGBA pixels will arrive.
fn request(display: &SCDisplay, bounds: PhysRect) -> mpsc::Receiver<Result<Vec<u8>>> {
    let (width, height) = (bounds.width as usize, bounds.height as usize);
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            display,
            &NSArray::new(),
        )
    };
    let configuration = unsafe { SCStreamConfiguration::new() };
    unsafe {
        configuration.setWidth(width);
        configuration.setHeight(height);
        configuration.setShowsCursor(false);
        configuration.setPixelFormat(PIXEL_FORMAT_BGRA);
        // The color space name is a constant, so it outlives the request.
        configuration.setColorSpaceName(kCGColorSpaceSRGB);
    }

    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        // The image is only borrowed for the call, so its pixels are copied
        // out here rather than on the waiting thread.
        let pixels = match unsafe { image.as_ref() } {
            Some(image) => read_pixels(image, width, height),
            None => Err(anyhow!("{}", describe(error))),
        };
        sender.send(pixels).ok();
    });
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            &filter,
            &configuration,
            Some(&handler),
        );
    }
    receiver
}

/// An image's pixels as tight RGBA, after checking it is the size asked for
/// and in the layout ScreenCaptureKit produces for BGRA screenshots.
fn read_pixels(image: &CGImage, width: usize, height: usize) -> Result<Vec<u8>> {
    let image = Some(image);
    let (actual_width, actual_height) = (CGImage::width(image), CGImage::height(image));
    if (actual_width, actual_height) != (width, height) {
        bail!("asked for a {width}×{height} image, got {actual_width}×{actual_height}");
    }
    let bits_per_pixel = CGImage::bits_per_pixel(image);
    let byte_order = CGImage::byte_order_info(image);
    let alpha = CGImage::alpha_info(image);
    if !is_bgra(bits_per_pixel, byte_order, alpha) {
        bail!("unexpected pixel layout: {bits_per_pixel} bits, {byte_order:?}, {alpha:?}");
    }
    let stride = CGImage::bytes_per_row(image);
    let data = CGImage::data_provider(image)
        .and_then(|provider| CGDataProvider::data(Some(&provider)))
        .context("the image has no pixel data")?;
    // The data is a copy nothing else holds, so it can't change while read.
    let bytes = unsafe { data.as_bytes_unchecked() };
    let row_bytes = width * 4;
    if stride < row_bytes || bytes.len() < stride * height.saturating_sub(1) + row_bytes {
        bail!(
            "a {width}×{height} image with {stride}-byte rows holds only {} bytes",
            bytes.len()
        );
    }
    Ok(bgra_to_rgba(bytes, width, height, stride))
}

/// Whether pixels are 32-bit little-endian with alpha (or padding) first,
/// which puts blue, green, red, alpha in memory order.
fn is_bgra(
    bits_per_pixel: usize,
    byte_order: CGImageByteOrderInfo,
    alpha: CGImageAlphaInfo,
) -> bool {
    bits_per_pixel == 32
        && byte_order == CGImageByteOrderInfo::Order32Little
        && [
            CGImageAlphaInfo::PremultipliedFirst,
            CGImageAlphaInfo::First,
            CGImageAlphaInfo::NoneSkipFirst,
        ]
        .contains(&alpha)
}

fn describe(error: *mut NSError) -> String {
    match unsafe { error.as_ref() } {
        Some(error) => error.localizedDescription().to_string(),
        None => "no reason given".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pixel_format_is_bgra_four_char_code() {
        assert_eq!(PIXEL_FORMAT_BGRA, 0x4247_5241);
    }

    #[test]
    fn test_only_little_endian_alpha_first_is_bgra() {
        let little = CGImageByteOrderInfo::Order32Little;
        assert!(is_bgra(32, little, CGImageAlphaInfo::PremultipliedFirst));
        assert!(is_bgra(32, little, CGImageAlphaInfo::NoneSkipFirst));
        assert!(
            !is_bgra(32, little, CGImageAlphaInfo::PremultipliedLast),
            "ABGR"
        );
        assert!(
            !is_bgra(
                32,
                CGImageByteOrderInfo::Order32Big,
                CGImageAlphaInfo::PremultipliedFirst
            ),
            "ARGB"
        );
        assert!(
            !is_bgra(64, little, CGImageAlphaInfo::PremultipliedFirst),
            "half floats"
        );
    }
}
