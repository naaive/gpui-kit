//! The system clipboard, for images as other applications expect them.
//!
//! GPUI's clipboard writes images as PNG only on Windows, which many
//! applications don't read, and doesn't write images on Linux at all. Snip
//! goes through `arboard` instead, which offers a DIB on Windows and serves
//! X11 and Wayland, behind [`ImageClipboard`] so tests use a fake.

use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{App, Global};
use image::RgbaImage;

/// What can be pinned from the clipboard.
#[derive(Clone, Debug, PartialEq)]
pub enum ClipboardContent {
    Image(RgbaImage),
    Text(String),
}

pub trait ImageClipboard: Send + Sync {
    fn write_image(&self, image: &RgbaImage) -> Result<()>;
    fn write_text(&self, text: &str) -> Result<()>;
    /// An image if the clipboard holds one, else its text, if any.
    fn read(&self) -> Result<Option<ClipboardContent>>;
}

/// The clipboard all of Snip uses.
#[derive(Clone)]
pub struct GlobalClipboard(pub Arc<dyn ImageClipboard>);

impl Global for GlobalClipboard {}

pub fn clipboard(cx: &App) -> Arc<dyn ImageClipboard> {
    cx.global::<GlobalClipboard>().0.clone()
}

/// The operating system's clipboard.
///
/// The `arboard` handle is opened once and kept: on X11 the process that
/// owns the selection must stay reachable to hand its contents out.
#[derive(Default)]
pub struct SystemClipboard {
    handle: Mutex<Option<arboard::Clipboard>>,
}

impl SystemClipboard {
    fn with<T>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> Result<T>) -> Result<T> {
        let mut handle = self
            .handle
            .lock()
            .map_err(|_| anyhow!("the clipboard is unavailable"))?;
        if handle.is_none() {
            *handle = Some(arboard::Clipboard::new().context("cannot open the clipboard")?);
        }
        let result = f(handle.as_mut().expect("opened above"));
        if result.is_err() {
            // A broken connection (a restarted X server, say) is reopened
            // on the next use.
            *handle = None;
        }
        result
    }
}

impl ImageClipboard for SystemClipboard {
    fn write_image(&self, image: &RgbaImage) -> Result<()> {
        self.with(|clipboard| {
            clipboard
                .set_image(arboard::ImageData {
                    width: image.width() as usize,
                    height: image.height() as usize,
                    bytes: image.as_raw().as_slice().into(),
                })
                .context("cannot copy the image")
        })
    }

    fn write_text(&self, text: &str) -> Result<()> {
        self.with(|clipboard| clipboard.set_text(text).context("cannot copy the text"))
    }

    fn read(&self) -> Result<Option<ClipboardContent>> {
        self.with(|clipboard| {
            if let Ok(image) = clipboard.get_image() {
                let image = RgbaImage::from_raw(
                    image.width as u32,
                    image.height as u32,
                    image.bytes.into_owned(),
                )
                .context("the clipboard image is malformed")?;
                return Ok(Some(ClipboardContent::Image(image)));
            }
            Ok(clipboard
                .get_text()
                .ok()
                .filter(|text| !text.trim().is_empty())
                .map(ClipboardContent::Text))
        })
    }
}

/// A clipboard of its own, for tests and previews, which leave the user's
/// clipboard alone.
#[cfg(any(test, feature = "preview"))]
#[derive(Default)]
pub struct MemoryClipboard {
    content: Mutex<Option<ClipboardContent>>,
}

#[cfg(any(test, feature = "preview"))]
impl MemoryClipboard {
    pub fn content(&self) -> Option<ClipboardContent> {
        self.content.lock().unwrap().clone()
    }
}

#[cfg(any(test, feature = "preview"))]
impl ImageClipboard for MemoryClipboard {
    fn write_image(&self, image: &RgbaImage) -> Result<()> {
        *self.content.lock().unwrap() = Some(ClipboardContent::Image(image.clone()));
        Ok(())
    }

    fn write_text(&self, text: &str) -> Result<()> {
        *self.content.lock().unwrap() = Some(ClipboardContent::Text(text.into()));
        Ok(())
    }

    fn read(&self) -> Result<Option<ClipboardContent>> {
        Ok(self.content())
    }
}
