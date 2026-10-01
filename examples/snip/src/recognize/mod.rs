//! Reading the text in a capture: the content of QR codes, or else the
//! words found by the platform's own text recognition.
//!
//! QR codes are decoded in Rust on every platform. Text is recognized by
//! `Windows.Media.Ocr` on Windows and by Vision on macOS, both offline and
//! in the languages the user has installed; on Linux by the `tesseract`
//! command when it is installed.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use anyhow::Result;
use image::RgbaImage;

/// What was read from an image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reading {
    /// The contents of the QR codes found, one per code.
    QrCodes(Vec<String>),
    /// Recognized text, one line per line found.
    Text(String),
    /// Nothing readable.
    Nothing,
}

impl Reading {
    /// The text to copy, if anything was read.
    pub fn text(&self) -> Option<String> {
        match self {
            Self::QrCodes(contents) => Some(contents.join("\n")),
            Self::Text(text) => Some(text.clone()),
            Self::Nothing => None,
        }
    }
}

/// Reads `image`: the contents of its QR codes if it shows any, otherwise
/// its text. Blocks; run it off the main thread.
pub fn read(image: &RgbaImage) -> Result<Reading> {
    let codes = qr_codes(image);
    if !codes.is_empty() {
        return Ok(Reading::QrCodes(codes));
    }
    let text = recognize_text(image)?;
    Ok(if text.trim().is_empty() {
        Reading::Nothing
    } else {
        Reading::Text(text)
    })
}

/// The contents of every QR code that decodes, in the order found.
fn qr_codes(image: &RgbaImage) -> Vec<String> {
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(
        image.width() as usize,
        image.height() as usize,
        |x, y| {
            let [r, g, b, _] = image.get_pixel(x as u32, y as u32).0;
            ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8
        },
    );
    prepared
        .detect_grids()
        .into_iter()
        .filter_map(|grid| grid.decode().ok().map(|(_, content)| content))
        .filter(|content| !content.is_empty())
        .collect()
}

#[cfg(target_os = "windows")]
fn recognize_text(image: &RgbaImage) -> Result<String> {
    windows::recognize(image)
}

#[cfg(target_os = "macos")]
fn recognize_text(image: &RgbaImage) -> Result<String> {
    macos::recognize(image)
}

#[cfg(target_os = "linux")]
fn recognize_text(image: &RgbaImage) -> Result<String> {
    linux::recognize(image)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn recognize_text(_: &RgbaImage) -> Result<String> {
    anyhow::bail!("text recognition isn't available on this platform")
}

/// Joins the words of one line. Scripts written without spaces between
/// words (Chinese, Japanese) are recognized word by word, or character by
/// character, so no space goes between two of their characters.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn join_words<'a>(words: impl IntoIterator<Item = &'a str>) -> String {
    let mut line = String::new();
    for word in words {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        let needs_space = match (line.chars().last(), word.chars().next()) {
            (Some(last), Some(first)) => !is_unspaced(last) && !is_unspaced(first),
            _ => false,
        };
        if needs_space {
            line.push(' ');
        }
        line.push_str(word);
    }
    line
}

/// Whether `c` belongs to a script written without spaces, or is its
/// punctuation.
fn is_unspaced(c: char) -> bool {
    matches!(c,
        '\u{2E80}'..='\u{9FFF}' // CJK radicals, kana, Han
        | '\u{AC00}'..='\u{D7AF}' // Hangul syllables
        | '\u{F900}'..='\u{FAFF}' // CJK compatibility ideographs
        | '\u{FF00}'..='\u{FFEF}' // full-width forms
        | '\u{20000}'..='\u{2FA1F}' // Han extensions
    )
}

/// The factor to scale an image of `width` × `height` by before
/// recognition: down to fit `max_side`, or up to give small text, such as
/// a menu's, enough pixels to be read.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn recognition_scale(width: u32, height: u32, max_side: u32) -> f32 {
    let longest = width.max(height).max(1);
    if longest > max_side {
        max_side as f32 / longest as f32
    } else if longest * 2 <= max_side && height < 400 {
        2.
    } else {
        1.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_words() {
        assert_eq!(join_words(["Hello", "world"]), "Hello world");
        assert_eq!(join_words(["你", "好", "世界"]), "你好世界");
        assert_eq!(join_words(["Snip", "截圖", "工具"]), "Snip截圖工具");
        assert_eq!(join_words(["版本", "0.6", "已發布。"]), "版本0.6已發布。");
        assert_eq!(join_words(["", " a ", "b"]), "a b");
    }

    #[test]
    fn test_recognition_scale() {
        assert_eq!(recognition_scale(5200, 100, 2600), 0.5);
        assert_eq!(recognition_scale(300, 40, 2600), 2.);
        assert_eq!(recognition_scale(1600, 900, 2600), 1., "large enough as is");
    }

    #[test]
    fn test_reads_a_qr_code() {
        let code = qrcode::QrCode::new("https://gpui-kit.com").unwrap();
        let modules = code.width() as u32;
        // Four pixels a module, with the four-module quiet zone around it.
        let side = (modules + 8) * 4;
        let colors = code.to_colors();
        let image = RgbaImage::from_fn(side, side, |x, y| {
            let (column, row) = ((x / 4) as i64 - 4, (y / 4) as i64 - 4);
            let is_dark = (0..modules as i64).contains(&column)
                && (0..modules as i64).contains(&row)
                && colors[(row as u32 * modules + column as u32) as usize] == qrcode::Color::Dark;
            let value = if is_dark { 0 } else { 255 };
            image::Rgba([value, value, value, 255])
        });
        assert_eq!(
            read(&image).unwrap(),
            Reading::QrCodes(vec!["https://gpui-kit.com".into()])
        );
    }

    #[test]
    fn test_reads_nothing_from_a_blank_image() {
        let image = RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]));
        assert!(qr_codes(&image).is_empty());
    }
}
