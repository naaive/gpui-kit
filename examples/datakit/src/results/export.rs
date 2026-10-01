//! Turning result rows into text other tools read.

use std::fmt::Write as _;

use datakit_driver::{ColumnInfo, Dialect, Row, TypeCategory, Value};

/// A text format rows can be copied or exported as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// Tab-separated, with a header row; what spreadsheets paste.
    Tsv,
    Csv,
    /// An array of objects, one per row.
    Json,
    /// One `INSERT` statement per row.
    SqlInsert,
    Markdown,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 5] = [
        ExportFormat::Tsv,
        ExportFormat::Csv,
        ExportFormat::Json,
        ExportFormat::SqlInsert,
        ExportFormat::Markdown,
    ];

    /// The file extension an export in this format is saved with.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Tsv => "tsv",
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::SqlInsert => "sql",
            ExportFormat::Markdown => "md",
        }
    }
}

/// Everything an export needs besides the rows.
pub struct ExportContext<'a> {
    pub columns: &'a [ColumnInfo],
    pub dialect: &'a dyn Dialect,
    /// The table `INSERT` statements name.
    pub table: &'a str,
}

pub fn export<'a>(
    format: ExportFormat,
    context: &ExportContext,
    rows: impl Iterator<Item = &'a Row>,
) -> String {
    match format {
        ExportFormat::Tsv => delimited(context, rows, '\t'),
        ExportFormat::Csv => delimited(context, rows, ','),
        ExportFormat::Json => json(context, rows),
        ExportFormat::SqlInsert => sql_insert(context, rows),
        ExportFormat::Markdown => markdown(context, rows),
    }
}

/// One value as the text a cell copy puts on the clipboard: `NULL` is empty.
pub fn plain_text(value: &Value) -> String {
    value
        .display()
        .map(|text| text.into_owned())
        .unwrap_or_default()
}

fn delimited<'a>(
    context: &ExportContext,
    rows: impl Iterator<Item = &'a Row>,
    separator: char,
) -> String {
    let quote = |field: &str| {
        if field.contains([separator, '"', '\n', '\r']) {
            format!("\"{}\"", field.replace('"', "\"\""))
        } else {
            field.to_string()
        }
    };
    let mut out = String::new();
    let header: Vec<String> = context.columns.iter().map(|c| quote(&c.name())).collect();
    out.push_str(&header.join(&separator.to_string()));
    out.push_str("\r\n");
    for row in rows {
        let fields: Vec<String> = row.iter().map(|value| quote(&plain_text(value))).collect();
        out.push_str(&fields.join(&separator.to_string()));
        out.push_str("\r\n");
    }
    out
}

fn json<'a>(context: &ExportContext, rows: impl Iterator<Item = &'a Row>) -> String {
    let mut out = String::from("[");
    for (row_ix, row) in rows.enumerate() {
        out.push_str(if row_ix == 0 { "\n  {" } else { ",\n  {" });
        for (col_ix, (column, value)) in context.columns.iter().zip(row.iter()).enumerate() {
            if col_ix > 0 {
                out.push_str(", ");
            }
            out.push_str(&json_string(&column.name()));
            out.push_str(": ");
            out.push_str(&json_value(value, column.category()));
        }
        out.push('}');
    }
    out.push_str(if out.len() == 1 { "]" } else { "\n]" });
    out
}

fn json_value(value: &Value, category: TypeCategory) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) if value.is_finite() => value.to_string(),
        Value::Float(value) => json_string(&value.to_string()),
        Value::Text(text) => match category {
            // A JSON column is embedded as JSON, not as a string of it.
            TypeCategory::Json if serde_json::from_str::<serde_json::Value>(text).is_ok() => {
                text.to_string()
            }
            TypeCategory::Decimal if text.parse::<f64>().is_ok_and(f64::is_finite) => {
                text.to_string()
            }
            _ => json_string(text),
        },
    }
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

fn sql_insert<'a>(context: &ExportContext, rows: impl Iterator<Item = &'a Row>) -> String {
    let columns: Vec<String> = context
        .columns
        .iter()
        .map(|column| context.dialect.quote_identifier(&column.name()))
        .collect();
    let mut out = String::new();
    for row in rows {
        let values: Vec<String> = row
            .iter()
            .zip(context.columns)
            .map(|(value, column)| sql_literal(value, column.category()))
            .collect();
        let _ = writeln!(
            out,
            "INSERT INTO {} ({}) VALUES ({});",
            context.table,
            columns.join(", "),
            values.join(", ")
        );
    }
    out
}

fn sql_literal(value: &Value, category: TypeCategory) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(true) => "TRUE".into(),
        Value::Bool(false) => "FALSE".into(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) if value.is_finite() => value.to_string(),
        Value::Text(text) if category == TypeCategory::Decimal && text.parse::<f64>().is_ok() => {
            text.to_string()
        }
        other => format!("'{}'", plain_text(other).replace('\'', "''")),
    }
}

fn markdown<'a>(context: &ExportContext, rows: impl Iterator<Item = &'a Row>) -> String {
    let cell = |text: &str| text.replace('|', "\\|").replace(['\n', '\r'], " ");
    let mut out = String::new();
    let header: Vec<String> = context.columns.iter().map(|c| cell(&c.name())).collect();
    let _ = writeln!(out, "| {} |", header.join(" | "));
    let rule: Vec<&str> = context
        .columns
        .iter()
        .map(|column| {
            if column.category().is_numeric() {
                "---:"
            } else {
                "---"
            }
        })
        .collect();
    let _ = writeln!(out, "| {} |", rule.join(" | "));
    for row in rows {
        let cells: Vec<String> = row.iter().map(|value| cell(&plain_text(value))).collect();
        let _ = writeln!(out, "| {} |", cells.join(" | "));
    }
    out
}

/// The rows as an Excel workbook: one sheet with a bold header row, and
/// numbers and booleans as Excel's own, so formulas can use them.
pub fn export_xlsx<'a>(
    context: &ExportContext,
    rows: impl Iterator<Item = &'a Row>,
) -> anyhow::Result<Vec<u8>> {
    use rust_xlsxwriter::{Format, Workbook};

    /// The most characters Excel keeps in a cell.
    const CELL_LIMIT: usize = 32_767;
    /// The most rows a sheet has, the header among them.
    const ROW_LIMIT: usize = 1_048_576;

    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    let bold = Format::new().set_bold();
    for (col, column) in context.columns.iter().enumerate() {
        sheet.write_string_with_format(0, col as u16, column.name().as_ref(), &bold)?;
    }
    for (ix, row) in rows.take(ROW_LIMIT - 1).enumerate() {
        let row_ix = ix as u32 + 1;
        for (col, value) in row.iter().enumerate() {
            let col = col as u16;
            match value {
                Value::Null => {}
                Value::Bool(value) => {
                    sheet.write_boolean(row_ix, col, *value)?;
                }
                Value::Int(value) => {
                    sheet.write_number(row_ix, col, *value as f64)?;
                }
                Value::Float(value) => {
                    sheet.write_number(row_ix, col, *value)?;
                }
                // `numeric` keeps the server's text; Excel wants the number.
                Value::Text(text)
                    if context.columns[col as usize].category().is_numeric()
                        && let Ok(number) = text.trim().parse::<f64>() =>
                {
                    sheet.write_number(row_ix, col, number)?;
                }
                Value::Text(text) => {
                    let text: String = text.chars().take(CELL_LIMIT).collect();
                    sheet.write_string(row_ix, col, text)?;
                }
            }
        }
    }
    sheet.set_freeze_panes(1, 0)?;
    sheet.autofit();
    Ok(workbook.save_to_buffer()?)
}

#[cfg(test)]
mod tests {
    use datakit_driver_postgres::PostgresDialect;

    use super::*;

    fn columns() -> Vec<ColumnInfo> {
        vec![
            ColumnInfo::new("id", "integer", TypeCategory::Integer),
            ColumnInfo::new("note", "text", TypeCategory::Text),
            ColumnInfo::new("price", "numeric", TypeCategory::Decimal),
            ColumnInfo::new("data", "jsonb", TypeCategory::Json),
        ]
    }

    fn rows() -> Vec<Row> {
        vec![
            vec![
                Value::Int(1),
                Value::Text("say \"hi\", then\nleave".into()),
                Value::Text("9.90".into()),
                Value::Text("{\"k\": [1, 2]}".into()),
            ]
            .into(),
            vec![
                Value::Int(2),
                Value::Null,
                Value::Null,
                Value::Text("it's".into()),
            ]
            .into(),
        ]
    }

    fn render(format: ExportFormat) -> String {
        let columns = columns();
        let context = ExportContext {
            columns: &columns,
            dialect: &PostgresDialect,
            table: "public.orders",
        };
        export(format, &context, rows().iter())
    }

    #[test]
    fn xlsx_is_a_workbook() {
        let columns = columns();
        let context = ExportContext {
            columns: &columns,
            dialect: &PostgresDialect,
            table: "public.orders",
        };
        let bytes = export_xlsx(&context, rows().iter()).unwrap();
        assert!(bytes.starts_with(b"PK"), "an xlsx file is a zip archive");
    }

    #[test]
    fn csv_quotes_only_fields_that_need_it() {
        assert_eq!(
            render(ExportFormat::Csv),
            "id,note,price,data\r\n\
             1,\"say \"\"hi\"\", then\nleave\",9.90,\"{\"\"k\"\": [1, 2]}\"\r\n\
             2,,,it's\r\n"
        );
    }

    #[test]
    fn json_keeps_numbers_numbers_and_embeds_json_columns() {
        assert_eq!(
            render(ExportFormat::Json),
            "[\n  {\"id\": 1, \"note\": \"say \\\"hi\\\", then\\nleave\", \"price\": 9.90, \
             \"data\": {\"k\": [1, 2]}},\n  {\"id\": 2, \"note\": null, \"price\": null, \
             \"data\": \"it's\"}\n]"
        );
    }

    #[test]
    fn an_empty_result_is_an_empty_json_array() {
        let columns = columns();
        let context = ExportContext {
            columns: &columns,
            dialect: &PostgresDialect,
            table: "t",
        };
        assert_eq!(export(ExportFormat::Json, &context, [].iter()), "[]");
    }

    #[test]
    fn inserts_quote_text_and_leave_numbers_bare() {
        let sql = render(ExportFormat::SqlInsert);
        // A newline inside a value stays inside its literal.
        let statements: Vec<&str> = sql.split_terminator(";\n").collect();
        assert_eq!(
            statements,
            [
                "INSERT INTO public.orders (id, note, price, data) VALUES \
                 (1, 'say \"hi\", then\nleave', 9.90, '{\"k\": [1, 2]}')",
                "INSERT INTO public.orders (id, note, price, data) VALUES (2, NULL, NULL, 'it''s')",
            ]
        );
    }

    #[test]
    fn markdown_escapes_pipes_and_aligns_numbers() {
        let markdown = render(ExportFormat::Markdown);
        let lines: Vec<&str> = markdown.lines().collect();
        assert_eq!(lines[0], "| id | note | price | data |");
        assert_eq!(lines[1], "| ---: | --- | ---: | --- |");
        assert_eq!(lines[3], "| 2 |  |  | it's |");
    }
}
