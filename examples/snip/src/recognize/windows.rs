//! Text recognition with `Windows.Media.Ocr`, in the languages of the
//! user's profile that have recognition installed.
//!
//! An engine reads one language. The profile's first language reads the
//! image; a Chinese, Japanese or Korean language further down the profile
//! reads it too, and its reading wins when it found characters of its
//! script, which the first language's engine would have misread.

use anyhow::{Context as _, Result};
use image::{RgbaImage, imageops::FilterType};
use windows::{
    Globalization::Language,
    Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap},
    Media::Ocr::OcrEngine,
    Storage::Streams::DataWriter,
    System::UserProfile::GlobalizationPreferences,
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
};

pub fn recognize(image: &RgbaImage) -> Result<String> {
    // The calling thread may not have joined an apartment; an error only
    // means it already has.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.ok();
    let engines = engines();
    if engines.is_empty() {
        anyhow::bail!(
            "no text recognition language is installed (Settings › Time & language › Language)"
        );
    }
    let max_side = OcrEngine::MaxImageDimension().unwrap_or(2600);
    let scale = super::recognition_scale(image.width(), image.height(), max_side);
    let scaled;
    let image = if scale == 1. {
        image
    } else {
        let width = ((image.width() as f32 * scale).round() as u32).clamp(1, max_side);
        let height = ((image.height() as f32 * scale).round() as u32).clamp(1, max_side);
        scaled = image::imageops::resize(image, width, height, FilterType::CatmullRom);
        &scaled
    };

    let bgra: Vec<u8> = image
        .pixels()
        .flat_map(|pixel| {
            let [r, g, b, a] = pixel.0;
            [b, g, r, a]
        })
        .collect();
    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        image.width() as i32,
        image.height() as i32,
        BitmapAlphaMode::Premultiplied,
    )?;
    let mut reading = String::new();
    for (ix, engine) in engines.iter().enumerate() {
        let text = read_with(engine, &bitmap)?;
        tracing::debug!(
            "{} reads {} characters in {}×{}",
            engine
                .RecognizerLanguage()
                .and_then(|language| language.LanguageTag())
                .map(|tag| tag.to_string_lossy())
                .unwrap_or_default(),
            text.chars().count(),
            image.width(),
            image.height()
        );
        if ix == 0 {
            reading = text;
        } else if text.chars().any(super::is_unspaced) {
            return Ok(text);
        }
    }
    Ok(reading)
}

/// The engine of the profile's first language that has recognition, then
/// one for each Chinese, Japanese or Korean language after it.
fn engines() -> Vec<OcrEngine> {
    let tags: Vec<String> = GlobalizationPreferences::Languages()
        .map(|languages| {
            languages
                .into_iter()
                .map(|tag| tag.to_string_lossy())
                .collect()
        })
        .unwrap_or_default();
    let mut engines = Vec::new();
    for tag in tags {
        let is_unspaced = ["zh", "ja", "ko", "yue"]
            .iter()
            .any(|prefix| tag == *prefix || tag.starts_with(&format!("{prefix}-")));
        if !engines.is_empty() && !is_unspaced {
            continue;
        }
        let Ok(language) = Language::CreateLanguage(&tag.into()) else {
            continue;
        };
        if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&language) {
            engines.push(engine);
        }
    }
    if engines.is_empty()
        && let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages()
    {
        engines.push(engine);
    }
    engines
}

/// What `engine` reads in `bitmap`, one line per line found.
fn read_with(engine: &OcrEngine, bitmap: &SoftwareBitmap) -> Result<String> {
    let result = engine
        .RecognizeAsync(bitmap)?
        .get()
        .context("text recognition failed")?;
    let mut lines = Vec::new();
    for line in result.Lines()? {
        let words: Vec<String> = line
            .Words()?
            .into_iter()
            .filter_map(|word| word.Text().ok())
            .map(|text| text.to_string_lossy())
            .collect();
        lines.push(super::join_words(words.iter().map(String::as_str)));
    }
    Ok(lines.join("\n"))
}
