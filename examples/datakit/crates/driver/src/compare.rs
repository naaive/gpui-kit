//! Comparing the rows of two tables, as DataGrip's Compare Content does.

use std::{collections::HashMap, sync::Arc};

use crate::{Row, RowChange, Value};

/// How the rows of a source table differ from those of a target: rows are
/// matched by their key, and compared on every column both tables have.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowsDiff {
    columns: Vec<Arc<str>>,
    key: Vec<usize>,
    only_in_source: Vec<Row>,
    only_in_target: Vec<Row>,
    /// Source and target versions of rows whose key matches.
    changed: Vec<(Row, Row)>,
    identical: usize,
}

impl RowsDiff {
    /// Compare `source` with `target`, both rows of `columns` in that order,
    /// matching rows on the columns at `key`. Rows that share a key with an
    /// earlier row of the same side are compared as if they were new.
    pub fn new(columns: Vec<Arc<str>>, key: Vec<usize>, source: &[Row], target: &[Row]) -> Self {
        let key_of = |row: &Row| -> Vec<Option<String>> {
            key.iter()
                .map(|ix| row[*ix].display().map(|text| text.into_owned()))
                .collect()
        };
        let mut targets: HashMap<Vec<Option<String>>, &Row> = HashMap::new();
        let mut only_in_target = Vec::new();
        for row in target {
            if let Some(duplicate) = targets.insert(key_of(row), row) {
                only_in_target.push(duplicate.clone());
            }
        }
        let mut diff = Self {
            columns,
            key: key.clone(),
            ..Self::default()
        };
        for row in source {
            match targets.remove(&key_of(row)) {
                Some(other) if same(row, other) => diff.identical += 1,
                Some(other) => diff.changed.push((row.clone(), other.clone())),
                None => diff.only_in_source.push(row.clone()),
            }
        }
        // What is left in the target matched nothing in the source.
        let mut rest: Vec<&Row> = targets.into_values().collect();
        rest.sort_by_key(|row| key_of(row));
        only_in_target.extend(rest.into_iter().cloned());
        diff.only_in_target = only_in_target;
        diff
    }

    pub fn columns(&self) -> &[Arc<str>] {
        &self.columns
    }

    pub fn only_in_source(&self) -> &[Row] {
        &self.only_in_source
    }

    pub fn only_in_target(&self) -> &[Row] {
        &self.only_in_target
    }

    pub fn changed(&self) -> &[(Row, Row)] {
        &self.changed
    }

    pub fn identical(&self) -> usize {
        self.identical
    }

    pub fn is_empty(&self) -> bool {
        self.only_in_source.is_empty() && self.only_in_target.is_empty() && self.changed.is_empty()
    }

    /// The changes that make the target's rows the source's: insert what
    /// only the source has, update what differs, delete what only the
    /// target has.
    pub fn changes_to_target(&self) -> Vec<RowChange> {
        let named = |row: &Row, columns: &[usize]| -> Vec<(Arc<str>, Value)> {
            columns
                .iter()
                .map(|ix| (self.columns[*ix].clone(), row[*ix].clone()))
                .collect()
        };
        let all: Vec<usize> = (0..self.columns.len()).collect();
        let mut changes: Vec<RowChange> = self
            .only_in_source
            .iter()
            .map(|row| RowChange::Insert {
                values: named(row, &all),
            })
            .collect();
        for (source, target) in &self.changed {
            let differing: Vec<usize> = all
                .iter()
                .copied()
                .filter(|ix| source[*ix] != target[*ix])
                .collect();
            changes.push(RowChange::Update {
                key: named(target, &self.key),
                values: named(source, &differing),
            });
        }
        changes.extend(self.only_in_target.iter().map(|row| RowChange::Delete {
            key: named(row, &self.key),
        }));
        changes
    }
}

fn same(a: &Row, b: &Row) -> bool {
    a.iter()
        .zip(b.iter())
        .all(|(a, b)| a.display() == b.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, name: &str) -> Row {
        vec![Value::Int(id), Value::Text(name.into())].into()
    }

    #[test]
    fn rows_are_matched_by_key_and_compared_by_value() {
        let source = [row(1, "Ada"), row(2, "Grace"), row(3, "Linus")];
        let target = [row(2, "Grace"), row(3, "Linus T."), row(4, "Ken")];
        let diff = RowsDiff::new(vec!["id".into(), "name".into()], vec![0], &source, &target);
        assert_eq!(diff.only_in_source(), [row(1, "Ada")]);
        assert_eq!(diff.only_in_target(), [row(4, "Ken")]);
        assert_eq!(diff.changed(), [(row(3, "Linus"), row(3, "Linus T."))]);
        assert_eq!(diff.identical(), 1);

        assert_eq!(
            diff.changes_to_target(),
            [
                RowChange::Insert {
                    values: vec![
                        ("id".into(), Value::Int(1)),
                        ("name".into(), Value::Text("Ada".into()))
                    ],
                },
                RowChange::Update {
                    key: vec![("id".into(), Value::Int(3))],
                    values: vec![("name".into(), Value::Text("Linus".into()))],
                },
                RowChange::Delete {
                    key: vec![("id".into(), Value::Int(4))],
                },
            ]
        );
    }
}
