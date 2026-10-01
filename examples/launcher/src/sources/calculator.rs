//! Arithmetic typed straight into the search field, and the conversions of
//! [`super::conversion`].
//!
//! The query is only treated as arithmetic when it contains an operation, so
//! typing `2024` or `pi` still searches for commands; `2024/12` or `sqrt(2)`
//! answers instead.

use std::{f64::consts, iter::Peekable, str::Chars};

use gpui_kit::SharedString;

use crate::model::{Accessory, Action, Effect, Item, ItemId};

pub const RESULT_ID: &str = "calculator/result";

/// The answer to `query` as an item, or `None` when it is not arithmetic.
pub fn item(query: &str) -> Option<Item> {
    let (answer, display): (SharedString, String) = match evaluate(query) {
        Some(value) => {
            let answer = format(value);
            (answer.clone().into(), answer)
        }
        None => {
            let conversion = super::conversion::convert(query)
                .or_else(|| super::currency::convert(query))
                .or_else(|| super::dates::answer(query))?;
            (conversion.value.into(), conversion.display)
        }
    };
    Some(
        Item::new(ItemId::new(RESULT_ID), format!("= {display}"))
            .with_subtitle(query.trim().to_owned())
            .with_icon("calculator")
            .with_accessory(Accessory::text("Calculator"))
            .with_action(Action::new("Copy Answer", Effect::Copy(answer.clone())))
            .with_action(Action::new("Paste Answer", Effect::Paste(answer))),
    )
}

/// Evaluates `expression`: `+ - * / % ^` (also `×`, `÷` and `**`),
/// parentheses, decimals, unary minus, the constants `pi` and `e`, and the
/// functions `sqrt`, `abs`, `ln`, `log`, `sin`, `cos`, `tan` (radians).
///
/// Returns `None` for anything else, for a lone number or constant, and for
/// results that are not finite, such as division by zero.
pub fn evaluate(expression: &str) -> Option<f64> {
    let mut parser = Parser {
        chars: expression.chars().peekable(),
        operations: 0,
    };
    let value = parser.expression()?;
    parser.skip_whitespace();
    (parser.chars.peek().is_none() && parser.operations > 0 && value.is_finite()).then_some(value)
}

/// Evaluates `expression` like [`evaluate`], but a plain number is an answer
/// too: the `5` of `5 km to mi`.
pub fn evaluate_value(expression: &str) -> Option<f64> {
    let mut parser = Parser {
        chars: expression.chars().peekable(),
        operations: 0,
    };
    let value = parser.expression()?;
    parser.skip_whitespace();
    (parser.chars.peek().is_none() && value.is_finite()).then_some(value)
}

/// The answer as a person would write it: no trailing zeros, no floating-point
/// noise (`0.1 + 0.2` is `0.3`), and scientific notation only for very large
/// or very small magnitudes.
pub fn format(value: f64) -> String {
    let magnitude = value.abs();
    if magnitude != 0.0 && !(1e-6..1e15).contains(&magnitude) {
        return format!("{value:e}");
    }
    let fixed = format!("{value:.10}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    match trimmed {
        "-0" => "0".to_owned(),
        _ => trimmed.to_owned(),
    }
}

/// A recursive-descent parser; each method is one precedence level.
struct Parser<'a> {
    chars: Peekable<Chars<'a>>,
    /// Binary operators and function calls seen; zero means the query was a
    /// plain number, which is a search, not a calculation.
    operations: usize,
}

impl Parser<'_> {
    fn skip_whitespace(&mut self) {
        while self.chars.next_if(|c| c.is_whitespace()).is_some() {}
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_whitespace();
        self.chars.peek().copied()
    }

    /// `term (('+' | '-') term)*`
    fn expression(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        while let Some(operator @ ('+' | '-')) = self.peek() {
            self.chars.next();
            let right = self.term()?;
            self.operations += 1;
            value = match operator {
                '+' => value + right,
                _ => value - right,
            };
        }
        Some(value)
    }

    /// `unary (('*' | '/' | '%') unary)*`
    fn term(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        loop {
            let operator = match self.peek() {
                Some('*' | '×') => {
                    self.chars.next();
                    '*'
                }
                Some(operator @ ('/' | '÷' | '%')) => {
                    self.chars.next();
                    operator
                }
                _ => return Some(value),
            };
            let right = self.unary()?;
            self.operations += 1;
            value = match operator {
                '*' => value * right,
                '%' => value % right,
                _ => value / right,
            };
        }
    }

    /// `('-' | '+') unary | power`; a sign binds looser than `^`, so `-2^2`
    /// is `-4` as in mathematics.
    fn unary(&mut self) -> Option<f64> {
        match self.peek()? {
            '-' | '−' => {
                self.chars.next();
                Some(-self.unary()?)
            }
            '+' => {
                self.chars.next();
                self.unary()
            }
            _ => self.power(),
        }
    }

    /// `primary ('^' | '**') unary`, right-associative.
    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        let is_power = match self.peek() {
            Some('^') => {
                self.chars.next();
                true
            }
            Some('*') => {
                let mut lookahead = self.chars.clone();
                lookahead.next();
                if lookahead.peek() == Some(&'*') {
                    self.chars.next();
                    self.chars.next();
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !is_power {
            return Some(base);
        }
        let exponent = self.unary()?;
        self.operations += 1;
        Some(base.powf(exponent))
    }

    /// A number, a parenthesized expression, a constant or a function call.
    fn primary(&mut self) -> Option<f64> {
        match self.peek()? {
            '(' => {
                self.chars.next();
                let value = self.expression()?;
                if self.peek()? != ')' {
                    return None;
                }
                self.chars.next();
                Some(value)
            }
            '0'..='9' | '.' => self.number(),
            'π' => {
                self.chars.next();
                Some(consts::PI)
            }
            c if c.is_ascii_alphabetic() => self.name(),
            _ => None,
        }
    }

    fn number(&mut self) -> Option<f64> {
        let mut text = String::new();
        while let Some(c) = self.chars.next_if(|c| c.is_ascii_digit() || *c == '.') {
            text.push(c);
        }
        text.parse().ok()
    }

    fn name(&mut self) -> Option<f64> {
        let mut name = String::new();
        while let Some(c) = self.chars.next_if(|c| c.is_ascii_alphanumeric()) {
            name.push(c.to_ascii_lowercase());
        }
        let function: fn(f64) -> f64 = match name.as_str() {
            "pi" => return Some(consts::PI),
            "e" => return Some(consts::E),
            "sqrt" => f64::sqrt,
            "abs" => f64::abs,
            "ln" => f64::ln,
            "log" => f64::log10,
            "sin" => f64::sin,
            "cos" => f64::cos,
            "tan" => f64::tan,
            _ => return None,
        };
        if self.peek()? != '(' {
            return None;
        }
        let argument = self.primary()?;
        self.operations += 1;
        Some(function(argument))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(expression: &str) -> Option<String> {
        evaluate(expression).map(format)
    }

    #[test]
    fn test_follows_precedence_and_associativity() {
        assert_eq!(answer("1 + 2 * 3").as_deref(), Some("7"));
        assert_eq!(answer("(1 + 2) * 3").as_deref(), Some("9"));
        assert_eq!(answer("10 - 4 - 3").as_deref(), Some("3"));
        assert_eq!(answer("2 ^ 3 ^ 2").as_deref(), Some("512"));
        assert_eq!(answer("2**10").as_deref(), Some("1024"));
        assert_eq!(answer("-2^2").as_deref(), Some("-4"));
        assert_eq!(answer("2^-1").as_deref(), Some("0.5"));
        assert_eq!(answer("7 % 4").as_deref(), Some("3"));
        assert_eq!(answer("6 × 7 ÷ 2").as_deref(), Some("21"));
        assert_eq!(answer("-(3 - 5)").as_deref(), Some("2"));
    }

    #[test]
    fn test_decimals_constants_and_functions() {
        assert_eq!(answer("0.1 + 0.2").as_deref(), Some("0.3"));
        assert_eq!(answer(".5 * 4").as_deref(), Some("2"));
        assert_eq!(answer("sqrt(16) + abs(-2)").as_deref(), Some("6"));
        assert_eq!(answer("2 * pi").as_deref(), Some("6.2831853072"));
        assert_eq!(answer("log(1000)").as_deref(), Some("3"));
        assert_eq!(answer("10^20 * 3").as_deref(), Some("3e20"));
    }

    #[test]
    fn test_only_calculations_are_answered() {
        for query in [
            "", "2024", "-5", "pi", "(3)", "chrome", "1 +", "2 3", "1/0", "sqrt 4", "1..2 + 1",
            "sqrt(-1)",
        ] {
            assert_eq!(evaluate(query), None, "{query:?}");
        }
    }

    #[test]
    fn test_result_item_copies_and_pastes_the_answer() {
        let item = item(" 6*7 ").unwrap();
        assert_eq!(item.title().as_ref(), "= 42");
        assert_eq!(item.subtitle().map(|s| s.as_ref()), Some("6*7"));
        assert!(
            matches!(item.primary_action().unwrap().effect(), Effect::Copy(text) if text == "42")
        );
        assert!(
            matches!(item.secondary_action().unwrap().effect(), Effect::Paste(text) if text == "42")
        );
    }
}
