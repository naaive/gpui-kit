//! Colors as people write them: `#f80`, `#ff8800`, `rgb(255 136 0)`,
//! `hsl(32, 100%, 50%)`, and back.

/// An sRGB color with alpha, 8 bits per channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

/// The ways a color can be written, in the order they are offered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Hex,
    Rgb,
    Hsl,
    /// `0.000 0.533 1.000`, as some design tools and shaders take it.
    Unit,
}

impl Format {
    pub const ALL: [Self; 4] = [Self::Hex, Self::Rgb, Self::Hsl, Self::Unit];

    pub fn title(self) -> &'static str {
        match self {
            Self::Hex => "HEX",
            Self::Rgb => "RGB",
            Self::Hsl => "HSL",
            Self::Unit => "Unit RGB",
        }
    }
}

impl Color {
    pub fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 255,
        }
    }

    /// As `0xRRGGBBAA`.
    pub fn rgba(self) -> u32 {
        u32::from_be_bytes([self.red, self.green, self.blue, self.alpha])
    }

    pub fn format(self, format: Format) -> String {
        let opaque = self.alpha == 255;
        let alpha = f32::from(self.alpha) / 255.;
        match format {
            Format::Hex => match opaque {
                true => format!("#{:02X}{:02X}{:02X}", self.red, self.green, self.blue),
                false => format!(
                    "#{:02X}{:02X}{:02X}{:02X}",
                    self.red, self.green, self.blue, self.alpha
                ),
            },
            Format::Rgb => match opaque {
                true => format!("rgb({}, {}, {})", self.red, self.green, self.blue),
                false => format!(
                    "rgba({}, {}, {}, {})",
                    self.red,
                    self.green,
                    self.blue,
                    trim(alpha)
                ),
            },
            Format::Hsl => {
                let (hue, saturation, lightness) = self.hsl();
                match opaque {
                    true => format!(
                        "hsl({}, {}%, {}%)",
                        hue.round(),
                        (saturation * 100.).round(),
                        (lightness * 100.).round()
                    ),
                    false => format!(
                        "hsla({}, {}%, {}%, {})",
                        hue.round(),
                        (saturation * 100.).round(),
                        (lightness * 100.).round(),
                        trim(alpha)
                    ),
                }
            }
            Format::Unit => {
                let unit = |channel: u8| format!("{:.3}", f32::from(channel) / 255.);
                let rgb = format!(
                    "{} {} {}",
                    unit(self.red),
                    unit(self.green),
                    unit(self.blue)
                );
                match opaque {
                    true => rgb,
                    false => format!("{rgb} {:.3}", alpha),
                }
            }
        }
    }

    /// Hue in degrees, saturation and lightness from 0 to 1.
    pub fn hsl(self) -> (f32, f32, f32) {
        let [r, g, b] = [self.red, self.green, self.blue].map(|c| f32::from(c) / 255.);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let lightness = (max + min) / 2.;
        let delta = max - min;
        if delta == 0. {
            return (0., 0., lightness);
        }
        let saturation = delta / (1. - (2. * lightness - 1.).abs());
        let hue = if max == r {
            60. * ((g - b) / delta).rem_euclid(6.)
        } else if max == g {
            60. * ((b - r) / delta + 2.)
        } else {
            60. * ((r - g) / delta + 4.)
        };
        (hue, saturation, lightness)
    }

    fn from_hsl(hue: f32, saturation: f32, lightness: f32, alpha: u8) -> Self {
        let chroma = (1. - (2. * lightness - 1.).abs()) * saturation;
        let h = hue.rem_euclid(360.) / 60.;
        let x = chroma * (1. - (h.rem_euclid(2.) - 1.).abs());
        let (r, g, b) = match h as u32 {
            0 => (chroma, x, 0.),
            1 => (x, chroma, 0.),
            2 => (0., chroma, x),
            3 => (0., x, chroma),
            4 => (x, 0., chroma),
            _ => (chroma, 0., x),
        };
        let m = lightness - chroma / 2.;
        let channel = |c: f32| ((c + m) * 255.).round().clamp(0., 255.) as u8;
        Self {
            red: channel(r),
            green: channel(g),
            blue: channel(b),
            alpha,
        }
    }

    /// Reads a color written as hex, `rgb()`/`rgba()` or `hsl()`/`hsla()`;
    /// `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        if let Some(hex) = text.strip_prefix('#') {
            return Self::from_hex(hex);
        }
        let (function, arguments) = text.strip_suffix(')')?.split_once('(')?;
        let arguments: Vec<&str> = arguments
            .split([',', ' ', '/'])
            .map(str::trim)
            .filter(|argument| !argument.is_empty())
            .collect();
        let alpha = |index: usize| -> Option<u8> {
            match arguments.get(index) {
                None => Some(255),
                Some(value) => {
                    let alpha = match value.strip_suffix('%') {
                        Some(percent) => percent.parse::<f32>().ok()? / 100.,
                        None => value.parse::<f32>().ok()?,
                    };
                    (0. ..=1.)
                        .contains(&alpha)
                        .then(|| (alpha * 255.).round() as u8)
                }
            }
        };
        match function.trim() {
            "rgb" | "rgba" if (3..=4).contains(&arguments.len()) => {
                let channel = |value: &str| -> Option<u8> {
                    match value.strip_suffix('%') {
                        Some(percent) => {
                            let percent = percent.parse::<f32>().ok()?;
                            (0. ..=100.)
                                .contains(&percent)
                                .then(|| (percent * 2.55).round() as u8)
                        }
                        None => value.parse::<u8>().ok(),
                    }
                };
                Some(Self {
                    red: channel(arguments[0])?,
                    green: channel(arguments[1])?,
                    blue: channel(arguments[2])?,
                    alpha: alpha(3)?,
                })
            }
            "hsl" | "hsla" if (3..=4).contains(&arguments.len()) => {
                let hue = arguments[0].trim_end_matches("deg").parse::<f32>().ok()?;
                let percent = |value: &str| -> Option<f32> {
                    let value = value.strip_suffix('%')?.parse::<f32>().ok()?;
                    (0. ..=100.).contains(&value).then_some(value / 100.)
                };
                Some(Self::from_hsl(
                    hue,
                    percent(arguments[1])?,
                    percent(arguments[2])?,
                    alpha(3)?,
                ))
            }
            _ => None,
        }
    }

    fn from_hex(hex: &str) -> Option<Self> {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let digit = |index: usize| u8::from_str_radix(&hex[index..=index], 16).ok();
        let pair = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
        Some(match hex.len() {
            3 | 4 => Self {
                red: digit(0)? * 17,
                green: digit(1)? * 17,
                blue: digit(2)? * 17,
                alpha: match hex.len() {
                    4 => digit(3)? * 17,
                    _ => 255,
                },
            },
            6 | 8 => Self {
                red: pair(0)?,
                green: pair(2)?,
                blue: pair(4)?,
                alpha: match hex.len() {
                    8 => pair(6)?,
                    _ => 255,
                },
            },
            _ => return None,
        })
    }
}

/// `0.5`, not `0.50`.
fn trim(value: f32) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_format_colors() {
        let orange = Color::rgb(255, 136, 0);
        assert_eq!(Color::parse("#f80"), Some(orange));
        assert_eq!(Color::parse("#FF8800"), Some(orange));
        assert_eq!(Color::parse("rgb(255, 136, 0)"), Some(orange));
        assert_eq!(Color::parse("rgb(255 136 0)"), Some(orange));
        assert_eq!(Color::parse("hsl(32, 100%, 50%)"), Some(orange));
        assert_eq!(
            Color::parse("rgba(255, 136, 0, 0.5)").map(|color| color.alpha),
            Some(128)
        );
        assert_eq!(
            Color::parse("#ff880080").map(|color| color.alpha),
            Some(128)
        );
        assert_eq!(Color::parse("#ggg"), None);
        assert_eq!(Color::parse("#12345"), None);
        assert_eq!(Color::parse("rgb(300, 0, 0)"), None);
        assert_eq!(Color::parse("hello"), None);

        assert_eq!(orange.format(Format::Hex), "#FF8800");
        assert_eq!(orange.format(Format::Rgb), "rgb(255, 136, 0)");
        assert_eq!(orange.format(Format::Hsl), "hsl(32, 100%, 50%)");
        assert_eq!(orange.format(Format::Unit), "1.000 0.533 0.000");
        let half = Color {
            alpha: 128,
            ..orange
        };
        assert_eq!(half.format(Format::Rgb), "rgba(255, 136, 0, 0.5)");
        assert_eq!(half.format(Format::Hex), "#FF880080");
        assert_eq!(orange.rgba(), 0xFF8800FF);
    }
}
