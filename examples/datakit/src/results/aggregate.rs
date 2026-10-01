use std::collections::HashSet;

use datakit_driver::Value;

/// What the values of a selected column add up to, as DataGrip's
/// aggregate view shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Aggregates {
    count: usize,
    nulls: usize,
    distinct: usize,
    numbers: Option<Numbers>,
}

/// Sums and bounds, when every value that is not `NULL` is a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Numbers {
    sum: f64,
    min: f64,
    max: f64,
    average: f64,
}

impl Aggregates {
    pub fn of<'a>(values: impl IntoIterator<Item = &'a Value>) -> Self {
        let mut count = 0;
        let mut nulls = 0;
        let mut distinct = HashSet::new();
        let mut numbers: Option<Vec<f64>> = Some(Vec::new());
        for value in values {
            count += 1;
            let Some(text) = value.display() else {
                nulls += 1;
                continue;
            };
            let number = match value {
                Value::Int(value) => Some(*value as f64),
                Value::Float(value) => Some(*value),
                // `numeric` keeps the server's text.
                Value::Text(text) => text.trim().parse::<f64>().ok(),
                Value::Bool(_) | Value::Null => None,
            };
            match (number, &mut numbers) {
                (Some(number), Some(numbers)) => numbers.push(number),
                _ => numbers = None,
            }
            distinct.insert(text.into_owned());
        }
        let numbers = numbers
            .filter(|numbers| !numbers.is_empty())
            .map(|numbers| {
                let sum: f64 = numbers.iter().sum();
                Numbers {
                    sum,
                    min: numbers.iter().copied().fold(f64::INFINITY, f64::min),
                    max: numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                    average: sum / numbers.len() as f64,
                }
            });
        Self {
            count,
            nulls,
            distinct: distinct.len(),
            numbers,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn nulls(&self) -> usize {
        self.nulls
    }

    pub fn distinct(&self) -> usize {
        self.distinct
    }

    pub fn numbers(&self) -> Option<&Numbers> {
        self.numbers.as_ref()
    }
}

impl Numbers {
    pub fn sum(&self) -> f64 {
        self.sum
    }

    pub fn min(&self) -> f64 {
        self.min
    }

    pub fn max(&self) -> f64 {
        self.max
    }

    pub fn average(&self) -> f64 {
        self.average
    }
}

/// A number as the aggregates show it: whole numbers without a fraction,
/// others to at most six places.
pub fn number_text(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{number:.0}")
    } else {
        let text = format!("{number:.6}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_add_up_and_text_counts() {
        let values = [
            Value::Int(1),
            Value::Text("2.5".into()),
            Value::Null,
            Value::Int(1),
        ];
        let aggregates = Aggregates::of(&values);
        assert_eq!(aggregates.count(), 4);
        assert_eq!(aggregates.nulls(), 1);
        assert_eq!(aggregates.distinct(), 2);
        let numbers = aggregates.numbers().unwrap();
        assert_eq!(number_text(numbers.sum()), "4.5");
        assert_eq!(number_text(numbers.average()), "1.5");
        assert_eq!(number_text(numbers.min()), "1");
        assert_eq!(number_text(numbers.max()), "2.5");

        let values = [Value::Int(1), Value::Text("one".into())];
        assert!(Aggregates::of(&values).numbers().is_none());
        assert!(Aggregates::of(&[Value::Null]).numbers().is_none());
    }
}
