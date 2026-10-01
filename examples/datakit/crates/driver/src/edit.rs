use std::sync::Arc;

use crate::Value;

/// One change to one row of a table, as the data editor collects them.
///
/// A row is found by its key: the values of the table's primary key, or of
/// every column when it has none.
#[derive(Clone, Debug, PartialEq)]
pub enum RowChange {
    Insert {
        values: Vec<(Arc<str>, Value)>,
    },
    Update {
        key: Vec<(Arc<str>, Value)>,
        values: Vec<(Arc<str>, Value)>,
    },
    Delete {
        key: Vec<(Arc<str>, Value)>,
    },
}
