//! Text recognition through the `tesseract` command, when it is installed:
//! Linux desktops have no recognition service of their own.

use std::{
    io::Write as _,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};
use image::RgbaImage;

pub fn recognize(image: &RgbaImage) -> Result<String> {
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("text recognition needs the tesseract command; install tesseract-ocr")?;
    child
        .stdin
        .take()
        .context("no input to tesseract")?
        .write_all(&png)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("tesseract couldn’t read the image");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}
