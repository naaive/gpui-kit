/// `statement` read through a filter: its rows that meet `condition`, in the
/// order `order_by` gives, as DataGrip's result filter runs them. The
/// statement becomes a derived table, so the condition and the order name
/// its result's columns. An empty condition or order is left out, and with
/// neither the statement is returned as it is.
///
/// The statement is put on lines of its own, so a line comment at its end
/// cannot swallow the closing parenthesis.
pub fn filtered_statement(statement: &str, condition: &str, order_by: &str) -> String {
    let statement = statement.trim().trim_end_matches(';').trim_end();
    let condition = condition.trim();
    let order_by = order_by.trim();
    if condition.is_empty() && order_by.is_empty() {
        return statement.to_string();
    }
    let mut sql = format!("SELECT * FROM (\n{statement}\n) filtered");
    if !condition.is_empty() {
        sql.push_str("\nWHERE ");
        sql.push_str(condition);
    }
    if !order_by.is_empty() {
        sql.push_str("\nORDER BY ");
        sql.push_str(order_by);
    }
    sql
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_statement_becomes_a_derived_table() {
        assert_eq!(
            filtered_statement("select * from orders -- all;\n;", "total > 10", "id desc"),
            "SELECT * FROM (\nselect * from orders -- all;\n) filtered\nWHERE total > 10\nORDER BY id desc"
        );
        assert_eq!(
            filtered_statement("select 1", "", " a "),
            "SELECT * FROM (\nselect 1\n) filtered\nORDER BY a"
        );
        assert_eq!(filtered_statement("select 1;", " ", ""), "select 1");
    }
}
