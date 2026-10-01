//! Currency conversion typed into the search field: `100 usd to twd`,
//! `€25 in yen`, `3000 jpy to eur`.
//!
//! Rates come from open.er-api.com (daily, no key) and are kept in the data
//! directory; they are fetched in the background when missing or older than
//! a day, so a conversion never waits on the network. Until the first fetch
//! succeeds there is no answer.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::{calculator::evaluate_value, conversion::Conversion};

const SOURCE: &str = "https://open.er-api.com/v6/latest/USD";
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Rates against the US dollar, and when they were fetched.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Rates {
    fetched_at: u64,
    rates: BTreeMap<String, f64>,
}

static RATES: Mutex<Option<Rates>> = Mutex::new(None);

fn path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("currency-rates.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Loads the saved rates and fetches new ones in the background when they
/// are missing or stale.
pub fn start() {
    std::thread::Builder::new()
        .name("currency-rates".into())
        .spawn(|| {
            let saved: Option<Rates> = path()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|text| serde_json::from_str(&text).ok());
            let fresh = saved.as_ref().is_some_and(|rates| {
                now().saturating_sub(rates.fetched_at) < STALE_AFTER.as_secs()
            });
            if let Some(saved) = saved
                && let Ok(mut rates) = RATES.lock()
            {
                *rates = Some(saved);
            }
            if fresh {
                return;
            }
            match fetch() {
                Ok(fetched) => {
                    if let (Some(path), Ok(json)) = (path(), serde_json::to_string(&fetched)) {
                        crate::search::write_snapshot(&path, &json).ok();
                    }
                    if let Ok(mut rates) = RATES.lock() {
                        *rates = Some(fetched);
                    }
                }
                Err(error) => tracing::info!("cannot fetch currency rates: {error:#}"),
            }
        })
        .ok();
}

fn fetch() -> anyhow::Result<Rates> {
    #[derive(Deserialize)]
    struct Response {
        result: String,
        rates: BTreeMap<String, f64>,
    }
    let response: Response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?
        .get(SOURCE)
        .send()?
        .error_for_status()?
        .json()?;
    anyhow::ensure!(
        response.result == "success",
        "the rates service answered {}",
        response.result
    );
    Ok(Rates {
        fetched_at: now(),
        rates: response.rates,
    })
}

/// The currency code a word names: a code (`usd`), a symbol or a name.
fn code(word: &str) -> Option<String> {
    let word = word.trim().to_lowercase();
    let named = match word.as_str() {
        "$" | "dollar" | "dollars" | "us$" => "USD",
        "€" | "euro" | "euros" => "EUR",
        "£" | "pound" | "pounds" | "quid" => "GBP",
        "¥" | "yen" | "円" | "日元" => "JPY",
        "yuan" | "rmb" | "元" | "人民币" | "人民幣" => "CNY",
        "ntd" | "nt$" | "nt" | "台币" | "台幣" | "新台幣" => "TWD",
        "hk$" => "HKD",
        "won" | "₩" | "韩元" | "韓元" => "KRW",
        "rupee" | "rupees" | "₹" => "INR",
        "franc" | "francs" => "CHF",
        "baht" | "฿" => "THB",
        "ruble" | "rubles" | "₽" => "RUB",
        code if code.len() == 3 && code.chars().all(|c| c.is_ascii_alphabetic()) => {
            return Some(code.to_uppercase());
        }
        _ => return None,
    };
    Some(named.to_owned())
}

/// Splits `€25` or `25 usd` into the amount and the currency.
fn amount_and_currency(source: &str) -> Option<(f64, String)> {
    let source = source.trim();
    // A leading symbol: `$5`, `€25`, `NT$100`.
    for symbol in ["nt$", "us$", "hk$", "$", "€", "£", "¥", "₩", "₹"] {
        if let Some(rest) = source.to_lowercase().strip_prefix(symbol) {
            return Some((evaluate_value(rest.trim())?, code(symbol)?));
        }
    }
    let split = source.rfind(|c: char| c.is_ascii_digit() || c == ')' || c == '.')? + 1;
    let (amount, currency) = (source[..split].trim(), source[split..].trim());
    Some((evaluate_value(amount)?, code(currency)?))
}

/// Converts `query` with `rates`, or `None` when it is not a conversion
/// between two currencies the rates know.
fn convert_with(query: &str, rates: &Rates) -> Option<Conversion> {
    // ASCII lowercasing keeps byte offsets, so they slice `query` safely.
    let lower = query.trim().to_ascii_lowercase();
    let (ix, length) = [" to ", " in ", " as "]
        .iter()
        .filter_map(|separator| lower.rfind(separator).map(|ix| (ix, separator.len())))
        .max_by_key(|(ix, _)| *ix)?;
    let (amount, from) = amount_and_currency(&query.trim()[..ix])?;
    let to = code(&lower[ix + length..])?;
    if from == to {
        return None;
    }
    let (from_rate, to_rate) = (rates.rates.get(&from)?, rates.rates.get(&to)?);
    let result = amount / from_rate * to_rate;
    let value = format!("{result:.2}");
    Some(Conversion {
        display: format!("{} {to}", group_thousands(&value)),
        value,
    })
}

pub fn convert(query: &str) -> Option<Conversion> {
    let rates = RATES.lock().ok()?;
    convert_with(query, rates.as_ref()?)
}

/// `1234567.89` → `1,234,567.89`.
fn group_thousands(value: &str) -> String {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let (sign, digits) = match whole.strip_prefix('-') {
        Some(digits) => ("-", digits),
        None => ("", whole),
    };
    let mut grouped = String::new();
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction.is_empty() {
        true => format!("{sign}{grouped}"),
        false => format!("{sign}{grouped}.{fraction}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rates() -> Rates {
        Rates {
            fetched_at: 0,
            rates: [("USD", 1.), ("TWD", 32.), ("EUR", 0.9), ("JPY", 150.)]
                .into_iter()
                .map(|(code, rate)| (code.to_owned(), rate))
                .collect(),
        }
    }

    fn display(query: &str) -> Option<String> {
        convert_with(query, &rates()).map(|conversion| conversion.display)
    }

    #[test]
    fn test_converts_between_currencies() {
        assert_eq!(display("100 usd to twd").as_deref(), Some("3,200.00 TWD"));
        assert_eq!(display("€90 in dollars").as_deref(), Some("100.00 USD"));
        assert_eq!(display("15000 yen to usd").as_deref(), Some("100.00 USD"));
        assert_eq!(display("10 台幣 to usd").as_deref(), Some("0.31 USD"));
        assert_eq!(display("(1+1) usd as eur").as_deref(), Some("1.80 EUR"));
        assert_eq!(display("100 usd to xyz"), None);
        assert_eq!(display("5 km to mi"), None);
        assert_eq!(
            display("1 \u{212A} to usd"),
            None,
            "the Kelvin sign must not panic"
        );
        assert_eq!(group_thousands("-1234567.5"), "-1,234,567.5");
    }
}
