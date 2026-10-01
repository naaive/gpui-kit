//! Wayland: the desktop's screenshot portal.
//!
//! The compositor composes the whole desktop into one image and hands back
//! a file. A non-interactive request shows no dialog where the user has
//! allowed screenshots; otherwise the desktop asks once.

use anyhow::{Context as _, Result};
use ashpd::desktop::screenshot::Screenshot;

use super::super::Frame;
use crate::geometry::{DisplayArea, PhysRect};

pub fn capture() -> Result<Vec<Frame>> {
    let uri = smol::block_on(async {
        let response = Screenshot::request()
            .interactive(false)
            .modal(false)
            .send()
            .await
            .context("the screenshot portal is unavailable")?
            .response()
            .context("the screenshot was refused")?;
        anyhow::Ok(response.uri().clone())
    })?;
    let path = file_path(uri.as_str())
        .with_context(|| format!("the screenshot portal returned {uri}, not a file"))?;
    let image = image::open(&path)
        .with_context(|| format!("cannot read the screenshot {}", path.display()))?
        .to_rgba8();
    // The file is ours to dispose of.
    std::fs::remove_file(&path).ok();
    let (width, height) = image.dimensions();
    // The image is in device pixels. Its scale is the compositor's; the
    // session corrects it against the display GPUI reports.
    let area = DisplayArea::new(PhysRect::new(0, 0, width as i32, height as i32), 1.);
    Ok(vec![Frame::new(area, 0, image.into_raw())?])
}

/// The local path a `file://` URI names, percent-decoded.
fn file_path(uri: &str) -> Option<std::path::PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    // An authority, if any, ends at the path's first slash.
    let encoded = &encoded[encoded.find('/')?..];
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] == b'%'
            && let Some(byte) = encoded
                .get(ix + 1..ix + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(byte);
            ix += 3;
        } else {
            decoded.push(bytes[ix]);
            ix += 1;
        }
    }
    Some(String::from_utf8(decoded).ok()?.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_path() {
        assert_eq!(
            file_path("file:///tmp/Screenshot%20from%202026.png"),
            Some("/tmp/Screenshot from 2026.png".into())
        );
        assert_eq!(
            file_path("file://localhost/tmp/a.png"),
            Some("/tmp/a.png".into())
        );
        assert_eq!(file_path("https://example.com/a.png"), None);
    }
}
