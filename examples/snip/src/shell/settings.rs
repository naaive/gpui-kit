//! Snip's settings, stored as `settings.json` in its data directory.
//!
//! A missing file means the defaults; an unreadable one is reported and the
//! defaults are used without overwriting it, so a hand edit with a typo is
//! not silently lost.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::{
    raster::{DEFAULT_NAME_TEMPLATE, ImageFormat},
    scene::{Color, PALETTE, Style},
};

pub const DEFAULT_CAPTURE_SHORTCUT: &str = "f1";
pub const DEFAULT_PIN_SHORTCUT: &str = "f3";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Follows the system's light or dark appearance.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn title(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    /// A GPUI keystroke, such as `f1` or `ctrl-shift-a`.
    capture_shortcut: String,
    pin_shortcut: String,
    /// Where Save puts images; the pictures folder when unset.
    save_directory: Option<PathBuf>,
    /// A `strftime` template for the names Save gives images.
    name_template: String,
    image_format: ImageFormat,
    copy_after_saving: bool,
    show_magnifier: bool,
    detect_windows: bool,
    appearance: Appearance,
    /// The last annotation color, as `#RRGGBB`.
    annotation_color: String,
    stroke_width: f32,
    font_size: f32,
}

impl Default for Settings {
    fn default() -> Self {
        let style = Style::default();
        Self {
            capture_shortcut: DEFAULT_CAPTURE_SHORTCUT.into(),
            pin_shortcut: DEFAULT_PIN_SHORTCUT.into(),
            save_directory: None,
            name_template: DEFAULT_NAME_TEMPLATE.into(),
            image_format: ImageFormat::default(),
            copy_after_saving: false,
            show_magnifier: true,
            detect_windows: true,
            appearance: Appearance::default(),
            annotation_color: style.color().hex(),
            stroke_width: style.stroke_width(),
            font_size: style.font_size(),
        }
    }
}

impl Settings {
    pub fn capture_shortcut(&self) -> &str {
        &self.capture_shortcut
    }

    pub fn pin_shortcut(&self) -> &str {
        &self.pin_shortcut
    }

    /// The folder Save writes to.
    pub fn save_directory(&self) -> PathBuf {
        self.save_directory
            .clone()
            .or_else(|| dirs::picture_dir().map(|pictures| pictures.join("Snip")))
            .or_else(|| dirs::home_dir().map(|home| home.join("Snip")))
            .unwrap_or_else(|| PathBuf::from("Snip"))
    }

    pub fn name_template(&self) -> &str {
        &self.name_template
    }

    pub fn image_format(&self) -> ImageFormat {
        self.image_format
    }

    pub fn is_copying_after_saving(&self) -> bool {
        self.copy_after_saving
    }

    pub fn is_showing_magnifier(&self) -> bool {
        self.show_magnifier
    }

    pub fn is_detecting_windows(&self) -> bool {
        self.detect_windows
    }

    pub fn appearance(&self) -> Appearance {
        self.appearance
    }

    /// The annotation style the next capture starts with.
    pub fn annotation_style(&self) -> Style {
        Style::default()
            .with_color(parse_hex(&self.annotation_color).unwrap_or(PALETTE[0]))
            .with_stroke_width(self.stroke_width)
            .with_font_size(self.font_size)
    }

    pub fn with_capture_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.capture_shortcut = shortcut.into();
        self
    }

    pub fn with_pin_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.pin_shortcut = shortcut.into();
        self
    }

    pub fn with_save_directory(mut self, directory: Option<PathBuf>) -> Self {
        self.save_directory = directory;
        self
    }

    pub fn with_name_template(mut self, template: impl Into<String>) -> Self {
        self.name_template = template.into();
        self
    }

    pub fn with_image_format(mut self, format: ImageFormat) -> Self {
        self.image_format = format;
        self
    }

    pub fn with_copy_after_saving(mut self, is_copying: bool) -> Self {
        self.copy_after_saving = is_copying;
        self
    }

    pub fn with_show_magnifier(mut self, is_showing: bool) -> Self {
        self.show_magnifier = is_showing;
        self
    }

    pub fn with_detect_windows(mut self, is_detecting: bool) -> Self {
        self.detect_windows = is_detecting;
        self
    }

    pub fn with_appearance(mut self, appearance: Appearance) -> Self {
        self.appearance = appearance;
        self
    }

    /// Remembers the color and sizes of `style` for the next capture.
    pub fn with_annotation_style(mut self, style: &Style) -> Self {
        self.annotation_color = style.color().hex();
        self.stroke_width = style.stroke_width();
        self.font_size = style.font_size();
        self
    }

    /// Reads settings; a missing file yields the defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid settings", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// Writes settings through a temporary file, so a crash mid-write never
    /// leaves a truncated `settings.json`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .with_context(|| format!("cannot create {}", directory.display()))?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text)
            .with_context(|| format!("cannot write {}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("cannot write {}", path.display()))
    }
}

/// Parses `#RRGGBB`.
fn parse_hex(text: &str) -> Option<Color> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(digits.get(range)?, 16).ok();
    Some(Color::rgb(channel(0..2)?, channel(2..4)?, channel(4..6)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip_and_atomic_write() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());

        let settings = Settings::default()
            .with_capture_shortcut("ctrl-shift-a")
            .with_image_format(ImageFormat::Jpeg)
            .with_detect_windows(false)
            .with_annotation_style(
                &Style::default()
                    .with_color(PALETTE[3])
                    .with_stroke_width(8.),
            );
        settings.save(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        let loaded = Settings::load(&path).unwrap();
        assert_eq!(loaded, settings);
        assert_eq!(loaded.annotation_style().color(), PALETTE[3]);
        assert_eq!(loaded.annotation_style().stroke_width(), 8.);
    }

    #[test]
    fn test_unknown_and_missing_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"capture_shortcut": "f2", "future": 1}"#).unwrap();
        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.capture_shortcut(), "f2");
        assert_eq!(settings.pin_shortcut(), DEFAULT_PIN_SHORTCUT);

        std::fs::write(&path, "{").unwrap();
        assert!(Settings::load(&path).is_err());
    }

    #[test]
    fn test_parse_hex() {
        assert_eq!(parse_hex("#1C7ED6"), Some(Color::rgb(0x1C, 0x7E, 0xD6)));
        assert_eq!(parse_hex("1C7ED6"), None);
        assert_eq!(parse_hex("#XYZXYZ"), None);
    }
}
