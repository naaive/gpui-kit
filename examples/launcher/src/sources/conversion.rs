//! Conversions typed into the search field: units (`5 km to mi`,
//! `100 f in c`), percentages (`20% of 150`, `80 - 15%`) and number bases
//! (`255 in hex`, `0xff to dec`).

use super::calculator::{Notation, evaluate_value, format};

/// What a unit measures; only units of one dimension convert.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dimension {
    Length,
    Mass,
    Temperature,
    Volume,
    Area,
    Data,
    Time,
    Speed,
}

/// A unit: its names, and its size in the dimension's base unit.
struct Unit {
    names: &'static [&'static str],
    /// Shown in the answer.
    symbol: &'static str,
    dimension: Dimension,
    factor: f64,
}

const fn unit(
    names: &'static [&'static str],
    symbol: &'static str,
    dimension: Dimension,
    factor: f64,
) -> Unit {
    Unit {
        names,
        symbol,
        dimension,
        factor,
    }
}

use Dimension::*;

const UNITS: &[Unit] = &[
    // Length, in metres.
    unit(
        &[
            "mm",
            "millimeter",
            "millimeters",
            "millimetre",
            "millimetres",
        ],
        "mm",
        Length,
        0.001,
    ),
    unit(
        &[
            "cm",
            "centimeter",
            "centimeters",
            "centimetre",
            "centimetres",
        ],
        "cm",
        Length,
        0.01,
    ),
    unit(
        &["m", "meter", "meters", "metre", "metres"],
        "m",
        Length,
        1.,
    ),
    unit(
        &["km", "kilometer", "kilometers", "kilometre", "kilometres"],
        "km",
        Length,
        1000.,
    ),
    unit(&["in", "inch", "inches", "\""], "in", Length, 0.0254),
    unit(&["ft", "foot", "feet", "'"], "ft", Length, 0.3048),
    unit(&["yd", "yard", "yards"], "yd", Length, 0.9144),
    unit(&["mi", "mile", "miles"], "mi", Length, 1609.344),
    unit(
        &["nmi", "nautical mile", "nautical miles"],
        "nmi",
        Length,
        1852.,
    ),
    // Mass, in grams.
    unit(&["mg", "milligram", "milligrams"], "mg", Mass, 0.001),
    unit(&["g", "gram", "grams"], "g", Mass, 1.),
    unit(
        &["kg", "kilogram", "kilograms", "kilo", "kilos"],
        "kg",
        Mass,
        1000.,
    ),
    unit(
        &["t", "tonne", "tonnes", "ton", "tons"],
        "t",
        Mass,
        1_000_000.,
    ),
    unit(&["oz", "ounce", "ounces"], "oz", Mass, 28.349_523_125),
    unit(&["lb", "lbs", "pound", "pounds"], "lb", Mass, 453.592_37),
    unit(&["st", "stone", "stones"], "st", Mass, 6350.293_18),
    unit(&["jin", "斤"], "斤", Mass, 500.),
    // Temperature: the factor is unused; see `to_kelvin`.
    unit(&["c", "°c", "celsius"], "°C", Temperature, 1.),
    unit(&["f", "°f", "fahrenheit"], "°F", Temperature, 1.),
    unit(&["k", "kelvin"], "K", Temperature, 1.),
    // Volume, in litres.
    unit(
        &[
            "ml",
            "milliliter",
            "milliliters",
            "millilitre",
            "millilitres",
        ],
        "ml",
        Volume,
        0.001,
    ),
    unit(
        &["l", "liter", "liters", "litre", "litres"],
        "l",
        Volume,
        1.,
    ),
    unit(
        &["tsp", "teaspoon", "teaspoons"],
        "tsp",
        Volume,
        0.004_928_921_59,
    ),
    unit(
        &["tbsp", "tablespoon", "tablespoons"],
        "tbsp",
        Volume,
        0.014_786_764_8,
    ),
    unit(
        &["floz", "fl oz", "fluid ounce", "fluid ounces"],
        "fl oz",
        Volume,
        0.029_573_529_6,
    ),
    unit(&["cup", "cups"], "cup", Volume, 0.236_588_236_5),
    unit(&["pt", "pint", "pints"], "pt", Volume, 0.473_176_473),
    unit(&["qt", "quart", "quarts"], "qt", Volume, 0.946_352_946),
    unit(&["gal", "gallon", "gallons"], "gal", Volume, 3.785_411_784),
    // Area, in square metres.
    unit(
        &["m2", "m²", "sqm", "square meter", "square meters"],
        "m²",
        Area,
        1.,
    ),
    unit(
        &["km2", "km²", "square kilometer", "square kilometers"],
        "km²",
        Area,
        1_000_000.,
    ),
    unit(
        &["ft2", "ft²", "sqft", "square foot", "square feet"],
        "ft²",
        Area,
        0.092_903_04,
    ),
    unit(&["acre", "acres"], "acre", Area, 4046.856_422_4),
    unit(&["ha", "hectare", "hectares"], "ha", Area, 10_000.),
    // Data, in bytes.
    unit(&["bit", "bits"], "bit", Data, 0.125),
    unit(&["b", "byte", "bytes"], "B", Data, 1.),
    unit(&["kb", "kilobyte", "kilobytes"], "KB", Data, 1e3),
    unit(&["mb", "megabyte", "megabytes"], "MB", Data, 1e6),
    unit(&["gb", "gigabyte", "gigabytes"], "GB", Data, 1e9),
    unit(&["tb", "terabyte", "terabytes"], "TB", Data, 1e12),
    unit(&["kib", "kibibyte", "kibibytes"], "KiB", Data, 1024.),
    unit(&["mib", "mebibyte", "mebibytes"], "MiB", Data, 1_048_576.),
    unit(
        &["gib", "gibibyte", "gibibytes"],
        "GiB",
        Data,
        1_073_741_824.,
    ),
    unit(
        &["tib", "tebibyte", "tebibytes"],
        "TiB",
        Data,
        1_099_511_627_776.,
    ),
    // Time, in seconds.
    unit(&["ms", "millisecond", "milliseconds"], "ms", Time, 0.001),
    unit(&["s", "sec", "secs", "second", "seconds"], "s", Time, 1.),
    unit(&["min", "mins", "minute", "minutes"], "min", Time, 60.),
    unit(&["h", "hr", "hrs", "hour", "hours"], "h", Time, 3600.),
    unit(&["d", "day", "days"], "d", Time, 86_400.),
    unit(&["wk", "week", "weeks"], "wk", Time, 604_800.),
    unit(&["yr", "year", "years"], "yr", Time, 31_557_600.),
    // Speed, in metres per second.
    unit(&["m/s", "mps"], "m/s", Speed, 1.),
    unit(&["km/h", "kmh", "kph"], "km/h", Speed, 1000. / 3600.),
    unit(&["mph"], "mph", Speed, 0.447_04),
    unit(&["kn", "knot", "knots"], "kn", Speed, 1852. / 3600.),
];

fn find_unit(name: &str) -> Option<&'static Unit> {
    let name = name.trim().to_lowercase();
    UNITS
        .iter()
        .find(|unit| unit.names.contains(&name.as_str()))
}

fn to_kelvin(value: f64, symbol: &str) -> f64 {
    match symbol {
        "°C" => value + 273.15,
        "°F" => (value - 32.) * 5. / 9. + 273.15,
        _ => value,
    }
}

fn from_kelvin(kelvin: f64, symbol: &str) -> f64 {
    match symbol {
        "°C" => kelvin - 273.15,
        "°F" => (kelvin - 273.15) * 9. / 5. + 32.,
        _ => kelvin,
    }
}

/// A conversion's answer: the value to copy, and how it reads.
#[derive(Debug, PartialEq)]
pub struct Conversion {
    pub value: String,
    pub display: String,
}

/// Converts `query`, its numbers written in `notation`, or `None` when it is
/// not a conversion.
pub fn convert(query: &str, notation: Notation) -> Option<Conversion> {
    let query = query.trim();
    percentage(query, notation)
        .or_else(|| base(query))
        .or_else(|| units(query, notation))
}

/// Splits `5 km to mi` at its last ` to ` or ` in `.
fn split_target(query: &str) -> Option<(&str, &str)> {
    // ASCII lowercasing keeps byte offsets, so they slice `query` safely.
    let lower = query.to_ascii_lowercase();
    [" to ", " in ", " as ", " → ", " -> "]
        .iter()
        .filter_map(|separator| lower.rfind(separator).map(|ix| (ix, separator.len())))
        .max_by_key(|(ix, _)| *ix)
        .map(|(ix, length)| (query[..ix].trim(), query[ix + length..].trim()))
}

fn units(query: &str, notation: Notation) -> Option<Conversion> {
    let (source, target) = split_target(query)?;
    // The amount is everything up to the last digit, mark or closing
    // parenthesis.
    let split = source.rfind(|c: char| c.is_ascii_digit() || matches!(c, ')' | '.' | ','))? + 1;
    let (amount, from) = (source[..split].trim(), source[split..].trim());
    let (from, to) = (find_unit(from)?, find_unit(target)?);
    if from.dimension != to.dimension || std::ptr::eq(from, to) {
        return None;
    }
    let amount = evaluate_value(amount, notation)?;
    let result = match from.dimension {
        Temperature => from_kelvin(to_kelvin(amount, from.symbol), to.symbol),
        _ => amount * from.factor / to.factor,
    };
    let value = format(round_significant(result), notation);
    Some(Conversion {
        display: format!("{value} {}", to.symbol),
        value,
    })
}

/// Rounds to 10 significant digits, so `5 km to mi` is `3.106855961`.
fn round_significant(value: f64) -> f64 {
    if value == 0. || !value.is_finite() {
        return value;
    }
    let digits = 10 - value.abs().log10().ceil() as i32;
    let scale = 10f64.powi(digits);
    let rounded = (value * scale).round() / scale;
    // Magnitudes near the ends of `f64` overflow the scale; keep them as is.
    match rounded.is_finite() {
        true => rounded,
        false => value,
    }
}

fn percentage(query: &str, notation: Notation) -> Option<Conversion> {
    let lower = query.to_lowercase();
    // `20% of 150`
    if let Some((percent, of)) = lower.split_once("% of ") {
        let value = evaluate_value(percent, notation)? / 100. * evaluate_value(of, notation)?;
        return Some(answer(value, notation));
    }
    // `150 + 20%`, `80 - 15%`
    let body = lower.strip_suffix('%')?;
    let ix = body.rfind(['+', '-'])?;
    let (base, percent) = (
        evaluate_value(&body[..ix], notation)?,
        evaluate_value(&body[ix + 1..], notation)?,
    );
    let value = match &body[ix..=ix] {
        "+" => base * (1. + percent / 100.),
        _ => base * (1. - percent / 100.),
    };
    Some(answer(value, notation))
}

fn answer(value: f64, notation: Notation) -> Conversion {
    let value = format(round_significant(value), notation);
    Conversion {
        display: value.clone(),
        value,
    }
}

fn base(query: &str) -> Option<Conversion> {
    let (source, target) = split_target(query)?;
    let source = source.trim().to_lowercase();
    let number = if let Some(hex) = source.strip_prefix("0x") {
        i128::from_str_radix(hex, 16).ok()?
    } else if let Some(binary) = source.strip_prefix("0b") {
        i128::from_str_radix(binary, 2).ok()?
    } else if let Some(octal) = source.strip_prefix("0o") {
        i128::from_str_radix(octal, 8).ok()?
    } else {
        source.parse::<i128>().ok()?
    };
    let value = match target.to_lowercase().as_str() {
        "hex" | "hexadecimal" => format!("0x{number:X}"),
        "bin" | "binary" => format!("0b{number:b}"),
        "oct" | "octal" => format!("0o{number:o}"),
        "dec" | "decimal" => number.to_string(),
        _ => return None,
    };
    Some(Conversion {
        display: value.clone(),
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(query: &str) -> Option<String> {
        convert(query, Notation::Dot).map(|conversion| conversion.display)
    }

    #[test]
    fn test_units() {
        assert_eq!(display("5 km to mi").as_deref(), Some("3.106855961 mi"));
        assert_eq!(display("100 F in C").as_deref(), Some("37.77777778 °C"));
        assert_eq!(display("0 c to k").as_deref(), Some("273.15 K"));
        assert_eq!(display("2+3 kg to lb").as_deref(), Some("11.02311311 lb"));
        assert_eq!(display("1.5GB in MiB").as_deref(), Some("1430.511475 MiB"));
        assert_eq!(display("90 min to h").as_deref(), Some("1.5 h"));
        assert_eq!(
            display("100 km/h to mph").as_deref(),
            Some("62.13711922 mph")
        );
        assert_eq!(display("5 km to kg"), None, "dimensions must agree");
        assert_eq!(display("go to settings"), None);
        assert_eq!(display("5 km"), None);
        assert_eq!(
            display("1 \u{212A} to c").as_deref(),
            Some("-272.15 °C"),
            "the Kelvin sign, shorter when lowercased, must not break slicing"
        );
    }

    #[test]
    fn test_percentages_and_bases() {
        assert_eq!(display("20% of 150").as_deref(), Some("30"));
        assert_eq!(display("100 + 10%").as_deref(), Some("110"));
        assert_eq!(display("80 - 15%").as_deref(), Some("68"));
        assert_eq!(display("255 in hex").as_deref(), Some("0xFF"));
        assert_eq!(display("0xff to dec").as_deref(), Some("255"));
        assert_eq!(display("10 to binary").as_deref(), Some("0b1010"));
        assert_eq!(display("10 to nothing"), None);
    }

    #[test]
    fn test_comma_notation() {
        let display = |query| convert(query, Notation::Comma).map(|conversion| conversion.display);
        assert_eq!(display("1,5 h to min").as_deref(), Some("90 min"));
        assert_eq!(display("90 min to h").as_deref(), Some("1,5 h"));
        assert_eq!(display("2,5% of 1.000").as_deref(), Some("25"));
    }
}
