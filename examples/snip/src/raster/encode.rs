//! Image files: formats and the names quick save gives them.

use std::{fmt::Write as _, io::Cursor, path::Path};

use anyhow::{Context as _, Result, bail};
use image::{ImageFormat as Codec, RgbaImage};
use serde::{Deserialize, Serialize};

/// The formats images are saved in.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
}

impl ImageFormat {
    pub const ALL: [Self; 2] = [Self::Png, Self::Jpeg];

    pub fn title(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    /// The format a path's extension names, for Save as….
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }
}

/// The file bytes of `image` in `format`.
pub fn encode(image: &RgbaImage, format: ImageFormat) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::new());
    match format {
        ImageFormat::Png => image.write_to(&mut bytes, Codec::Png)?,
        // JPEG has no alpha channel.
        ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(image.clone())
            .to_rgb8()
            .write_to(&mut bytes, Codec::Jpeg)?,
    }
    Ok(bytes.into_inner())
}

/// Writes `image` to `path` through a temporary file, so a failed write
/// never leaves a truncated image behind.
pub fn write(image: &RgbaImage, format: ImageFormat, path: &Path) -> Result<()> {
    let bytes = encode(image, format)?;
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)
            .with_context(|| format!("cannot create {}", directory.display()))?;
    }
    let temporary = path.with_extension("snip-partial");
    std::fs::write(&temporary, bytes)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("cannot write {}", path.display()))
}

/// The default file name template, in `strftime` form.
pub const DEFAULT_NAME_TEMPLATE: &str = "Snip_%Y-%m-%d_%H-%M-%S";

/// A file name from `template` at `time`, without extension. Characters
/// that file systems reject become `-`.
pub fn file_name(template: &str, time: chrono::DateTime<chrono::Local>) -> Result<String> {
    let mut name = String::new();
    write!(name, "{}", time.format(template))
        .ok()
        .with_context(|| format!("“{template}” isn't a valid name template"))?;
    let name: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim().trim_end_matches('.').to_owned();
    if name.is_empty() {
        bail!("The name template gives an empty file name.");
    }
    Ok(name)
}

/// A path in `directory` for a new image named from `template`, adding
/// ` (2)`, ` (3)`… when a file of that name already exists.
pub fn unused_path(
    directory: &Path,
    template: &str,
    format: ImageFormat,
    time: chrono::DateTime<chrono::Local>,
) -> Result<std::path::PathBuf> {
    let stem = file_name(template, time)?;
    let extension = format.extension();
    let mut path = directory.join(format!("{stem}.{extension}"));
    let mut number = 2;
    while path.exists() {
        path = directory.join(format!("{stem} ({number}).{extension}"));
        number += 1;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    fn noon() -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 10, 1, 12, 34, 56)
            .unwrap()
    }

    #[test]
    fn test_file_name_template() {
        assert_eq!(
            file_name(DEFAULT_NAME_TEMPLATE, noon()).unwrap(),
            "Snip_2026-10-01_12-34-56"
        );
        assert_eq!(
            file_name("%H:%M/shot?", noon()).unwrap(),
            "12-34-shot-",
            "separators file systems reject are replaced"
        );
        assert!(file_name("%Q", noon()).is_err(), "unknown specifier");
        assert!(file_name("  ", noon()).is_err());
    }

    #[test]
    fn test_unused_path_counts_up() {
        let directory = tempfile::tempdir().unwrap();
        let first = unused_path(directory.path(), "shot", ImageFormat::Png, noon()).unwrap();
        assert_eq!(first.file_name().unwrap(), "shot.png");
        std::fs::write(&first, b"").unwrap();
        let second = unused_path(directory.path(), "shot", ImageFormat::Png, noon()).unwrap();
        assert_eq!(second.file_name().unwrap(), "shot (2).png");
    }

    #[test]
    fn test_encode_round_trip() {
        let image = RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        let png = encode(&image, ImageFormat::Png).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(decoded, image);

        let jpeg = encode(&image, ImageFormat::Jpeg).unwrap();
        assert_eq!(image::guess_format(&jpeg).unwrap(), Codec::Jpeg);
    }

    #[test]
    fn test_format_from_path() {
        assert_eq!(
            ImageFormat::from_path(Path::new("a/b.JPEG")),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(ImageFormat::from_path(Path::new("b.gif")), None);
    }
}
