//! Edits to a table's rows that have not been written yet.
//!
//! The data editor shows the rows as the server sent them with these edits
//! laid over them; submitting turns them into statements through the
//! dialect, and reverting forgets them. Nothing here touches the grid or the
//! database.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use datakit_driver::{ColumnInfo, Dialect, Row, RowChange, Value};

/// The pending edits of one table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeSet {
    /// New values of existing rows, by row and column.
    updates: BTreeMap<usize, BTreeMap<usize, Value>>,
    /// Existing rows to delete.
    deletions: BTreeSet<usize>,
    /// New rows: a value for each column the person set.
    insertions: Vec<BTreeMap<usize, Value>>,
}

/// What a displayed row is, after the pending edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Unchanged,
    Updated,
    Deleted,
    Inserted,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.updates.is_empty() && self.deletions.is_empty() && self.insertions.is_empty()
    }

    /// How many rows the changes touch.
    pub fn len(&self) -> usize {
        self.updates.len() + self.deletions.len() + self.insertions.len()
    }

    pub fn inserted_rows(&self) -> usize {
        self.insertions.len()
    }

    /// The state of row `row`: an existing row below `existing`, or an
    /// inserted one above it.
    pub fn row_state(&self, row: usize, existing: usize) -> RowState {
        if row >= existing {
            RowState::Inserted
        } else if self.deletions.contains(&row) {
            RowState::Deleted
        } else if self.updates.contains_key(&row) {
            RowState::Updated
        } else {
            RowState::Unchanged
        }
    }

    /// Whether the cell has a value that is not written yet.
    pub fn is_edited(&self, row: usize, column: usize, existing: usize) -> bool {
        if row >= existing {
            return self
                .insertions
                .get(row - existing)
                .is_some_and(|values| values.contains_key(&column));
        }
        self.updates
            .get(&row)
            .is_some_and(|values| values.contains_key(&column))
    }

    /// The value the cell shows: the pending one, or the server's.
    pub fn value<'a>(&'a self, rows: &'a [Row], row: usize, column: usize) -> Option<&'a Value> {
        if row >= rows.len() {
            return self
                .insertions
                .get(row - rows.len())
                .and_then(|values| values.get(&column));
        }
        self.updates
            .get(&row)
            .and_then(|values| values.get(&column))
            .or_else(|| rows[row].get(column))
    }

    /// Set a cell. Setting an existing cell back to the server's value
    /// forgets the edit.
    pub fn set(&mut self, rows: &[Row], row: usize, column: usize, value: Value) {
        if row >= rows.len() {
            if let Some(values) = self.insertions.get_mut(row - rows.len()) {
                values.insert(column, value);
            }
            return;
        }
        let original = rows[row].get(column);
        let values = self.updates.entry(row).or_default();
        if original == Some(&value) {
            values.remove(&column);
        } else {
            values.insert(column, value);
        }
        if values.is_empty() {
            self.updates.remove(&row);
        }
    }

    /// Add an empty row; columns not set take their defaults.
    pub fn insert_row(&mut self) {
        self.insertions.push(BTreeMap::new());
    }

    /// Mark rows for deletion. An inserted row is simply removed; deleting a
    /// row that is already marked unmarks it.
    pub fn toggle_delete(&mut self, rows: &[usize], existing: usize) {
        let mut inserted: Vec<usize> = Vec::new();
        for &row in rows {
            if row >= existing {
                inserted.push(row - existing);
            } else if !self.deletions.remove(&row) {
                self.deletions.insert(row);
            }
        }
        inserted.sort_unstable();
        for ix in inserted.into_iter().rev() {
            if ix < self.insertions.len() {
                self.insertions.remove(ix);
            }
        }
    }

    /// Forget the edits of one row.
    pub fn revert_row(&mut self, row: usize, existing: usize) {
        if row >= existing {
            if row - existing < self.insertions.len() {
                self.insertions.remove(row - existing);
            }
        } else {
            self.updates.remove(&row);
            self.deletions.remove(&row);
        }
    }

    /// The changes as row changes, in the order they must run: deletions,
    /// then updates, then insertions. `key` lists the columns that identify
    /// a row.
    pub fn row_changes(
        &self,
        columns: &[ColumnInfo],
        key: &[usize],
        rows: &[Row],
    ) -> Vec<RowChange> {
        let name = |column: usize| -> Arc<str> { columns[column].name() };
        let key_of = |row: usize| -> Vec<(Arc<str>, Value)> {
            key.iter()
                .map(|&column| (name(column), rows[row][column].clone()))
                .collect()
        };
        let mut changes = Vec::new();
        for &row in &self.deletions {
            changes.push(RowChange::Delete { key: key_of(row) });
        }
        for (&row, values) in &self.updates {
            if self.deletions.contains(&row) {
                continue;
            }
            changes.push(RowChange::Update {
                key: key_of(row),
                values: values
                    .iter()
                    .map(|(&column, value)| (name(column), value.clone()))
                    .collect(),
            });
        }
        for values in &self.insertions {
            changes.push(RowChange::Insert {
                values: values
                    .iter()
                    .map(|(&column, value)| (name(column), value.clone()))
                    .collect(),
            });
        }
        changes
    }

    /// The statements that write the changes to `schema.relation`.
    pub fn statements(
        &self,
        dialect: &dyn Dialect,
        schema: &str,
        relation: &str,
        columns: &[ColumnInfo],
        key: &[usize],
        rows: &[Row],
    ) -> Vec<String> {
        self.row_changes(columns, key, rows)
            .iter()
            .map(|change| dialect.row_change(schema, relation, change))
            .collect()
    }
}

/// The value a person typed into a cell of a column of `category`: a
/// number for numeric columns when it reads as one, text otherwise.
pub fn typed_value(text: &str, column: &ColumnInfo) -> Value {
    use datakit_driver::TypeCategory;
    match column.category() {
        TypeCategory::Integer => text
            .trim()
            .parse()
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Text(text.into())),
        TypeCategory::Float => text
            .trim()
            .parse()
            .map(Value::Float)
            .unwrap_or_else(|_| Value::Text(text.into())),
        TypeCategory::Boolean => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "t" | "1" | "yes" => Value::Bool(true),
            "false" | "f" | "0" | "no" => Value::Bool(false),
            _ => Value::Text(text.into()),
        },
        _ => Value::Text(text.into()),
    }
}

#[cfg(test)]
mod tests {
    use datakit_driver::TypeCategory;
    use datakit_driver_postgres::PostgresDialect;

    use super::*;

    fn columns() -> Vec<ColumnInfo> {
        vec![
            ColumnInfo::new("id", "integer", TypeCategory::Integer),
            ColumnInfo::new("name", "text", TypeCategory::Text),
        ]
    }

    fn rows() -> Vec<Row> {
        vec![
            vec![Value::Int(1), Value::Text("Ada".into())].into(),
            vec![Value::Int(2), Value::Text("Grace".into())].into(),
        ]
    }

    #[test]
    fn edits_become_statements_in_a_safe_order() {
        let rows = rows();
        let mut changes = ChangeSet::default();
        changes.set(&rows, 0, 1, Value::Text("Ada L.".into()));
        changes.toggle_delete(&[1], rows.len());
        changes.insert_row();
        changes.set(&rows, 2, 1, Value::Text("Linus".into()));
        assert_eq!(changes.len(), 3);
        let statements = changes.statements(
            &PostgresDialect,
            "public",
            "people",
            &columns(),
            &[0],
            &rows,
        );
        assert_eq!(
            statements,
            vec![
                "DELETE FROM public.people WHERE id = 2",
                "UPDATE public.people SET name = 'Ada L.' WHERE id = 1",
                "INSERT INTO public.people (name) VALUES ('Linus')",
            ]
        );
    }

    #[test]
    fn setting_a_cell_back_forgets_the_edit() {
        let rows = rows();
        let mut changes = ChangeSet::default();
        changes.set(&rows, 0, 1, Value::Text("x".into()));
        assert_eq!(changes.row_state(0, 2), RowState::Updated);
        changes.set(&rows, 0, 1, Value::Text("Ada".into()));
        assert!(changes.is_empty());
    }

    #[test]
    fn deleting_twice_restores_and_deleting_a_new_row_drops_it() {
        let rows = rows();
        let mut changes = ChangeSet::default();
        changes.toggle_delete(&[0], 2);
        assert_eq!(changes.row_state(0, 2), RowState::Deleted);
        changes.toggle_delete(&[0], 2);
        assert_eq!(changes.row_state(0, 2), RowState::Unchanged);
        changes.insert_row();
        changes.insert_row();
        changes.toggle_delete(&[2, 3], 2);
        assert!(changes.is_empty());
        let _ = rows;
    }

    #[test]
    fn typed_text_becomes_the_columns_kind_of_value() {
        let id = ColumnInfo::new("id", "integer", TypeCategory::Integer);
        assert_eq!(typed_value(" 42 ", &id), Value::Int(42));
        assert_eq!(typed_value("4x", &id), Value::Text("4x".into()));
        let flag = ColumnInfo::new("ok", "boolean", TypeCategory::Boolean);
        assert_eq!(typed_value("yes", &flag), Value::Bool(true));
    }
}
