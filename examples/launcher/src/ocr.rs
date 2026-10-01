//! Reading the text in an image, with the system's own recognizer, so copied
//! images and screenshots can be found by the words in them.
//!
//! Windows has one built in (`Windows.Media.Ocr`) for the languages the user
//! installed; elsewhere nothing is recognized.

use std::path::Path;

/// The text in the image at `path`, in reading order; `None` when there is
/// none or the platform cannot tell. Blocking, and slow on large images:
/// call it on a background thread.
pub fn recognize(path: &Path) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows::recognize(path).map(|text| join_ideographs(&text))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        None
    }
}

/// The recognizer separates every Chinese or Japanese character with a
/// space, as if each were a word; this takes those spaces out again.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn join_ideographs(text: &str) -> String {
    let is_ideograph = |c: char| {
        matches!(c as u32,
            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF
                | 0x3000..=0x303F)
    };
    let chars: Vec<char> = text.chars().collect();
    let mut joined = String::with_capacity(text.len());
    for (index, c) in chars.iter().enumerate() {
        if *c == ' '
            && index > 0
            && is_ideograph(chars[index - 1])
            && chars.get(index + 1).is_some_and(|next| is_ideograph(*next))
        {
            continue;
        }
        joined.push(*c);
    }
    joined
}

#[cfg(target_os = "windows")]
mod windows {
    use std::path::Path;

    use ::windows::{
        Graphics::Imaging::{BitmapDecoder, BitmapPixelFormat, SoftwareBitmap},
        Media::Ocr::OcrEngine,
        Storage::{FileAccessMode, StorageFile},
        Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
        core::HSTRING,
    };

    pub fn recognize(path: &Path) -> Option<String> {
        let absolute = std::path::absolute(path).ok()?;
        let run = || -> ::windows::core::Result<String> {
            // Harmless when the thread is already initialized.
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
            }
            let file =
                StorageFile::GetFileFromPathAsync(&HSTRING::from(absolute.as_os_str()))?.get()?;
            let stream = file.OpenAsync(FileAccessMode::Read)?.get()?;
            let decoder = BitmapDecoder::CreateAsync(&stream)?.get()?;
            let bitmap = decoder.GetSoftwareBitmapAsync()?.get()?;
            // The recognizer takes 8-bit grayscale or BGRA.
            let bitmap = match bitmap.BitmapPixelFormat()? {
                BitmapPixelFormat::Bgra8 | BitmapPixelFormat::Gray8 => bitmap,
                _ => SoftwareBitmap::Convert(&bitmap, BitmapPixelFormat::Bgra8)?,
            };
            let largest = OcrEngine::MaxImageDimension()?;
            if bitmap.PixelWidth()? as u32 > largest || bitmap.PixelHeight()? as u32 > largest {
                return Ok(String::new());
            }
            // Each recognizer reads one language's script; the one that
            // reads the most is the image's language.
            let length = |text: &str| text.chars().filter(|c| !c.is_whitespace()).count();
            let mut best = String::new();
            for language in OcrEngine::AvailableRecognizerLanguages()?
                .into_iter()
                .take(4)
            {
                let Ok(engine) = OcrEngine::TryCreateFromLanguage(&language) else {
                    continue;
                };
                let result = engine.RecognizeAsync(&bitmap)?.get()?;
                let mut lines = Vec::new();
                for line in result.Lines()? {
                    lines.push(line.Text()?.to_string_lossy());
                }
                let text = lines.join("\n");
                if length(&text) > length(&best) {
                    best = text;
                }
            }
            Ok(best)
        };
        match run() {
            Ok(text) if !text.trim().is_empty() => Some(text),
            Ok(_) => None,
            Err(error) => {
                tracing::debug!("cannot read text in {}: {error}", path.display());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_ideographs() {
        assert_eq!(
            join_ideographs("你 好 世 界 hello world"),
            "你好世界 hello world"
        );
        assert_eq!(join_ideographs("a b"), "a b");
    }
}

#[cfg(all(test, target_os = "windows"))]
mod sample {
    /// Reads `OCR_SAMPLE`, when set, to try the recognizer by hand.
    #[test]
    #[ignore = "needs an image and the system recognizer"]
    fn test_recognize_sample() {
        let path = std::env::var("OCR_SAMPLE").unwrap();
        println!("{:?}", super::recognize(std::path::Path::new(&path)));
    }
}
