use std::path::PathBuf;

use gpui_kit::SharedString;

/// A picture beside an item, in a grid cell, or on an action.
#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    /// A Lucide icon name, such as `globe`, tinted by the theme.
    Icon(SharedString),
    /// An image file drawn as is, such as an application icon.
    File(PathBuf),
    /// A character drawn as text, such as an emoji.
    Glyph(SharedString),
    /// A swatch of a color, as `0xRRGGBBAA`.
    Color(u32),
}

impl Image {
    /// Reads the spelling extensions use: a path when it names a file
    /// (contains a path separator or has an image extension), otherwise a
    /// Lucide icon name.
    pub fn parse(value: &str) -> Self {
        let is_file = value.contains('/')
            || value.contains('\\')
            || [".png", ".jpg", ".jpeg", ".svg", ".gif", ".webp"]
                .iter()
                .any(|extension| value.to_ascii_lowercase().ends_with(extension));
        if is_file {
            Self::File(PathBuf::from(value))
        } else {
            Self::Icon(value.to_owned().into())
        }
    }
}

/// A semantic color for tags and accessories. Pages never name colors; the
/// theme decides what each tone looks like.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

impl Tone {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "neutral" => Self::Neutral,
            "accent" => Self::Accent,
            "success" => Self::Success,
            "warning" => Self::Warning,
            "danger" => Self::Danger,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_parse_tells_files_from_icon_names() {
        assert_eq!(Image::parse("globe"), Image::Icon("globe".into()));
        assert_eq!(
            Image::parse("assets/logo.png"),
            Image::File(PathBuf::from("assets/logo.png"))
        );
        assert_eq!(
            Image::parse("logo.SVG"),
            Image::File(PathBuf::from("logo.SVG"))
        );
    }
}
