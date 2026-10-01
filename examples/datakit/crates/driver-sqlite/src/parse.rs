//! What SQLite keeps only in the text of `CREATE` statements.
//!
//! The pragmas describe columns, indexes and foreign keys, but not constraint
//! names, `CHECK` expressions, `AUTOINCREMENT`, generation expressions, a
//! view's query or when a trigger fires. Those are read from the statement
//! `sqlite_schema` stores, which SQLite has already accepted, so a small
//! tokenizer that respects quotes, comments and parentheses is enough.

use std::ops::Range;

/// One token of SQL text, by its byte range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Token {
    shape: Shape,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// A bare word: a keyword or an unquoted name.
    Word,
    /// A quoted name: `"a"`, `` `a` `` or `[a]`.
    Quoted,
    /// A string or blob literal.
    Literal,
    Number,
    /// Any other single character, such as `(`, `,` or `.`.
    Symbol(char),
}

fn tokenize(sql: &str) -> Vec<Token> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut ix = 0;
    // The end of a quoted run that starts at `ix` and closes with `close`,
    // where a doubled `close` is an escaped one.
    let quoted = |ix: usize, close: u8| {
        let mut end = ix + 1;
        while end < bytes.len() {
            if bytes[end] == close {
                if close != b']' && bytes.get(end + 1) == Some(&close) {
                    end += 2;
                    continue;
                }
                return end + 1;
            }
            end += 1;
        }
        bytes.len()
    };
    while ix < bytes.len() {
        let byte = bytes[ix];
        let start = ix;
        let shape = match byte {
            _ if byte.is_ascii_whitespace() => {
                ix += 1;
                continue;
            }
            b'-' if bytes.get(ix + 1) == Some(&b'-') => {
                ix = sql[ix..].find('\n').map_or(bytes.len(), |end| ix + end + 1);
                continue;
            }
            b'/' if bytes.get(ix + 1) == Some(&b'*') => {
                ix = sql[ix + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| ix + 2 + end + 2);
                continue;
            }
            b'\'' => {
                ix = quoted(ix, b'\'');
                Shape::Literal
            }
            b'x' | b'X' if bytes.get(ix + 1) == Some(&b'\'') => {
                ix = quoted(ix + 1, b'\'');
                Shape::Literal
            }
            b'"' | b'`' => {
                ix = quoted(ix, byte);
                Shape::Quoted
            }
            b'[' => {
                ix = quoted(ix, b']');
                Shape::Quoted
            }
            b'0'..=b'9' => {
                ix += 1;
                while ix < bytes.len() {
                    let next = bytes[ix];
                    let exponent_sign = matches!(next, b'+' | b'-')
                        && matches!(bytes[ix - 1], b'e' | b'E')
                        && !sql[start..ix].starts_with("0x");
                    if next.is_ascii_alphanumeric() || next == b'.' || exponent_sign {
                        ix += 1;
                    } else {
                        break;
                    }
                }
                Shape::Number
            }
            _ if byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80 => {
                ix += 1;
                while ix < bytes.len()
                    && (bytes[ix].is_ascii_alphanumeric()
                        || matches!(bytes[ix], b'_' | b'$')
                        || bytes[ix] >= 0x80)
                {
                    ix += 1;
                }
                Shape::Word
            }
            _ => {
                let character = sql[ix..].chars().next().unwrap_or('?');
                ix += character.len_utf8();
                Shape::Symbol(character)
            }
        };
        tokens.push(Token {
            shape,
            start,
            end: ix.min(bytes.len()),
        });
    }
    tokens
}

/// Tokens of `sql`, with what they mean.
struct Tokens<'a> {
    sql: &'a str,
    tokens: Vec<Token>,
}

impl<'a> Tokens<'a> {
    fn new(sql: &'a str) -> Self {
        Self {
            sql,
            tokens: tokenize(sql),
        }
    }

    fn text(&self, ix: usize) -> &'a str {
        let token = self.tokens[ix];
        &self.sql[token.start..token.end]
    }

    /// Whether token `ix` is the keyword `word`.
    fn is_word(&self, ix: usize, word: &str) -> bool {
        self.tokens
            .get(ix)
            .is_some_and(|token| token.shape == Shape::Word)
            && self.text(ix).eq_ignore_ascii_case(word)
    }

    fn is_symbol(&self, ix: usize, symbol: char) -> bool {
        self.tokens
            .get(ix)
            .is_some_and(|token| token.shape == Shape::Symbol(symbol))
    }

    /// Token `ix` as a name, unquoted.
    fn name(&self, ix: usize) -> Option<String> {
        let token = self.tokens.get(ix)?;
        let text = self.text(ix);
        match token.shape {
            Shape::Word => Some(text.to_string()),
            Shape::Quoted | Shape::Literal => Some(unquote(text)),
            _ => None,
        }
    }

    /// The index just past the parenthesis that closes the one at `open`.
    fn skip_group(&self, open: usize) -> usize {
        let mut depth = 0usize;
        for ix in open..self.tokens.len() {
            if self.is_symbol(ix, '(') {
                depth += 1;
            } else if self.is_symbol(ix, ')') {
                depth -= 1;
                if depth == 0 {
                    return ix + 1;
                }
            }
        }
        self.tokens.len()
    }

    /// The source text of tokens `range`, as written.
    fn source(&self, range: Range<usize>) -> &'a str {
        if range.is_empty() {
            return "";
        }
        let start = self.tokens[range.start].start;
        let end = self.tokens[range.end - 1].end;
        &self.sql[start..end]
    }

    /// The text inside the parentheses that open at `open`.
    fn group_source(&self, open: usize) -> &'a str {
        let close = self.skip_group(open);
        self.source(open + 1..close.saturating_sub(1).max(open + 1))
    }

    /// The token ranges of the comma-separated items of `range`, splitting
    /// only at commas outside parentheses.
    fn items(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let mut items = Vec::new();
        let mut start = range.start;
        let mut ix = range.start;
        while ix < range.end {
            if self.is_symbol(ix, '(') {
                ix = self.skip_group(ix);
                continue;
            }
            if self.is_symbol(ix, ',') {
                items.push(start..ix);
                start = ix + 1;
            }
            ix += 1;
        }
        if start < range.end {
            items.push(start..range.end);
        }
        items
    }

    /// The first token at or after `from` that is the keyword `word`, not
    /// looking inside parentheses.
    fn find_word(&self, from: usize, to: usize, word: &str) -> Option<usize> {
        let mut ix = from;
        while ix < to {
            if self.is_symbol(ix, '(') {
                ix = self.skip_group(ix);
                continue;
            }
            if self.is_word(ix, word) {
                return Some(ix);
            }
            ix += 1;
        }
        None
    }

    /// The index after `CREATE [TEMP] <object> [IF NOT EXISTS]`, where the
    /// object's name starts; `None` when `sql` does not create `object`.
    fn after_create(&self, object: &str) -> Option<usize> {
        let mut ix = 0;
        if !self.is_word(ix, "CREATE") {
            return None;
        }
        ix += 1;
        while ["TEMP", "TEMPORARY", "UNIQUE", "VIRTUAL"]
            .iter()
            .any(|word| self.is_word(ix, word))
        {
            ix += 1;
        }
        if !self.is_word(ix, object) {
            return None;
        }
        ix += 1;
        if self.is_word(ix, "IF") && self.is_word(ix + 1, "NOT") && self.is_word(ix + 2, "EXISTS") {
            ix += 3;
        }
        Some(ix)
    }

    /// The index after the possibly qualified name that starts at `ix`.
    fn skip_name(&self, ix: usize) -> usize {
        if self.is_symbol(ix + 1, '.') {
            ix + 3
        } else {
            ix + 1
        }
    }

    /// The column names of the parenthesized list at `open`, each item's
    /// first token.
    fn names_in_group(&self, open: usize) -> Vec<String> {
        let close = self.skip_group(open).saturating_sub(1);
        self.items(open + 1..close)
            .into_iter()
            .filter_map(|item| self.name(item.start))
            .collect()
    }
}

/// The words of `sql` outside parentheses, upper-cased, in order. Comments,
/// quoted names and literals are not words.
pub(crate) fn top_level_words(sql: &str) -> Vec<String> {
    let tokens = Tokens::new(sql);
    let mut words = Vec::new();
    let mut ix = 0;
    while ix < tokens.tokens.len() {
        if tokens.is_symbol(ix, '(') {
            ix = tokens.skip_group(ix);
            continue;
        }
        if tokens.tokens[ix].shape == Shape::Word {
            words.push(tokens.text(ix).to_ascii_uppercase());
        }
        ix += 1;
    }
    words
}

/// A name or string as written, without its quotes.
fn unquote(text: &str) -> String {
    let mut chars = text.chars();
    let (Some(open), Some(close)) = (chars.next(), chars.next_back()) else {
        return text.to_string();
    };
    match (open, close) {
        ('"', '"') | ('`', '`') | ('\'', '\'') => {
            let inner = &text[1..text.len() - 1];
            inner.replace(&format!("{open}{open}"), &open.to_string())
        }
        ('[', ']') => text[1..text.len() - 1].to_string(),
        _ => text.to_string(),
    }
}

/// What a `CREATE TABLE` statement says beyond what the pragmas report.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct TableText {
    pub(crate) columns: Vec<ColumnText>,
    pub(crate) constraints: Vec<ConstraintText>,
    pub(crate) without_rowid: bool,
}

/// What the definition of one column says.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ColumnText {
    pub(crate) name: String,
    pub(crate) autoincrement: bool,
    /// The expression of a generated column.
    pub(crate) generated: Option<String>,
}

/// A constraint, written on a column or on the table.
#[derive(Debug, PartialEq)]
pub(crate) struct ConstraintText {
    /// The name after `CONSTRAINT`, when one was given.
    pub(crate) name: Option<String>,
    pub(crate) rule: RuleText,
}

#[derive(Debug, PartialEq)]
pub(crate) enum RuleText {
    PrimaryKey { columns: Vec<String> },
    Unique { columns: Vec<String> },
    ForeignKey { columns: Vec<String> },
    Check { expression: String },
}

impl TableText {
    pub(crate) fn column(&self, name: &str) -> Option<&ColumnText> {
        self.columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(name))
    }

    /// The name given to the constraint whose rule is `matches` and whose
    /// columns are `columns`.
    pub(crate) fn constraint_name(
        &self,
        columns: &[String],
        matches: impl Fn(&RuleText) -> Option<&Vec<String>>,
    ) -> Option<&str> {
        self.constraints.iter().find_map(|constraint| {
            let named = matches(&constraint.rule)?;
            let same = named.len() == columns.len()
                && named
                    .iter()
                    .zip(columns)
                    .all(|(a, b)| a.eq_ignore_ascii_case(b));
            same.then_some(constraint.name.as_deref()).flatten()
        })
    }
}

/// Read a `CREATE TABLE` statement. A statement this cannot read, such as
/// `CREATE TABLE … AS SELECT`, says nothing.
pub(crate) fn table(sql: &str) -> TableText {
    let tokens = Tokens::new(sql);
    let Some(name) = tokens.after_create("TABLE") else {
        return TableText::default();
    };
    let open = tokens.skip_name(name);
    if !tokens.is_symbol(open, '(') {
        return TableText::default();
    }
    let close = tokens.skip_group(open);
    let mut text = TableText {
        without_rowid: (close..tokens.tokens.len())
            .any(|ix| tokens.is_word(ix, "WITHOUT") && tokens.is_word(ix + 1, "ROWID")),
        ..Default::default()
    };
    for item in tokens.items(open + 1..close.saturating_sub(1)) {
        let first = item.start;
        let is_table_constraint = ["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"]
            .iter()
            .any(|word| tokens.is_word(first, word));
        if is_table_constraint {
            table_constraint(&tokens, item, &mut text);
        } else {
            column_definition(&tokens, item, &mut text);
        }
    }
    text
}

fn table_constraint(tokens: &Tokens, item: Range<usize>, text: &mut TableText) {
    let mut ix = item.start;
    let mut name = None;
    if tokens.is_word(ix, "CONSTRAINT") {
        name = tokens.name(ix + 1);
        ix += 2;
    }
    let group = |from: usize| (from..item.end).find(|&ix| tokens.is_symbol(ix, '('));
    let rule = if tokens.is_word(ix, "PRIMARY") {
        let Some(open) = group(ix) else { return };
        let close = tokens.skip_group(open);
        if (open..close).any(|ix| tokens.is_word(ix, "AUTOINCREMENT"))
            && let Some(column) = tokens.name(open + 1)
            && let Some(column) = text
                .columns
                .iter_mut()
                .find(|c| c.name.eq_ignore_ascii_case(&column))
        {
            column.autoincrement = true;
        }
        RuleText::PrimaryKey {
            columns: tokens.names_in_group(open),
        }
    } else if tokens.is_word(ix, "UNIQUE") {
        let Some(open) = group(ix) else { return };
        RuleText::Unique {
            columns: tokens.names_in_group(open),
        }
    } else if tokens.is_word(ix, "CHECK") {
        let Some(open) = group(ix) else { return };
        RuleText::Check {
            expression: tokens.group_source(open).to_string(),
        }
    } else if tokens.is_word(ix, "FOREIGN") {
        let Some(open) = group(ix) else { return };
        RuleText::ForeignKey {
            columns: tokens.names_in_group(open),
        }
    } else {
        return;
    };
    text.constraints.push(ConstraintText { name, rule });
}

fn column_definition(tokens: &Tokens, item: Range<usize>, text: &mut TableText) {
    let Some(name) = tokens.name(item.start) else {
        return;
    };
    let mut column = ColumnText {
        name: name.clone(),
        ..Default::default()
    };
    let mut pending_name = None;
    let mut ix = item.start + 1;
    while ix < item.end {
        if tokens.is_symbol(ix, '(') {
            ix = tokens.skip_group(ix);
            continue;
        }
        if tokens.is_word(ix, "CONSTRAINT") {
            pending_name = tokens.name(ix + 1);
            ix += 2;
            continue;
        }
        let rule = if tokens.is_word(ix, "PRIMARY") && tokens.is_word(ix + 1, "KEY") {
            Some(RuleText::PrimaryKey {
                columns: vec![name.clone()],
            })
        } else if tokens.is_word(ix, "UNIQUE") {
            Some(RuleText::Unique {
                columns: vec![name.clone()],
            })
        } else if tokens.is_word(ix, "REFERENCES") {
            Some(RuleText::ForeignKey {
                columns: vec![name.clone()],
            })
        } else if tokens.is_word(ix, "CHECK") && tokens.is_symbol(ix + 1, '(') {
            Some(RuleText::Check {
                expression: tokens.group_source(ix + 1).to_string(),
            })
        } else {
            if tokens.is_word(ix, "AUTOINCREMENT") {
                column.autoincrement = true;
            } else if tokens.is_word(ix, "AS") && tokens.is_symbol(ix + 1, '(') {
                column.generated = Some(tokens.group_source(ix + 1).to_string());
            }
            None
        };
        if let Some(rule) = rule {
            text.constraints.push(ConstraintText {
                name: pending_name.take(),
                rule,
            });
        }
        ix += 1;
    }
    text.columns.push(column);
}

/// The query of a `CREATE VIEW` statement: everything after `AS`.
pub(crate) fn view_query(sql: &str) -> Option<String> {
    let tokens = Tokens::new(sql);
    let name = tokens.after_create("VIEW")?;
    let after_name = tokens.skip_name(name);
    let as_ix = tokens.find_word(after_name, tokens.tokens.len(), "AS")?;
    let query = tokens.source(as_ix + 1..tokens.tokens.len());
    Some(query.trim().trim_end_matches(';').trim_end().to_string())
}

/// What an index statement says about its keys and rows.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct IndexText {
    /// Each key as written, without `ASC` or `DESC`.
    pub(crate) keys: Vec<String>,
    /// The condition of a partial index.
    pub(crate) predicate: Option<String>,
}

pub(crate) fn index(sql: &str) -> IndexText {
    let tokens = Tokens::new(sql);
    let Some(name) = tokens.after_create("INDEX") else {
        return IndexText::default();
    };
    let end = tokens.tokens.len();
    let Some(open) = (tokens.skip_name(name)..end).find(|&ix| tokens.is_symbol(ix, '(')) else {
        return IndexText::default();
    };
    let close = tokens.skip_group(open);
    let keys = tokens
        .items(open + 1..close.saturating_sub(1))
        .into_iter()
        .map(|mut item| {
            if tokens.is_word(item.end - 1, "ASC") || tokens.is_word(item.end - 1, "DESC") {
                item.end -= 1;
            }
            tokens.source(item).to_string()
        })
        .collect();
    let predicate = tokens.find_word(close, end, "WHERE").map(|ix| {
        tokens
            .source(ix + 1..end)
            .trim_end_matches(';')
            .trim_end()
            .to_string()
    });
    IndexText { keys, predicate }
}

/// When a trigger fires, as `CREATE TRIGGER` says it and in the form the
/// catalog uses: `AFTER UPDATE FOR EACH ROW`. SQLite triggers fire for each
/// row, before the event unless the statement says otherwise.
pub(crate) fn trigger_timing(sql: &str) -> String {
    let tokens = Tokens::new(sql);
    let Some(name) = tokens.after_create("TRIGGER") else {
        return String::new();
    };
    let mut ix = tokens.skip_name(name);
    let timing = if tokens.is_word(ix, "BEFORE") || tokens.is_word(ix, "AFTER") {
        ix += 1;
        tokens.text(ix - 1).to_ascii_uppercase()
    } else if tokens.is_word(ix, "INSTEAD") {
        ix += 2;
        "INSTEAD OF".to_string()
    } else {
        "BEFORE".to_string()
    };
    let event = tokens
        .tokens
        .get(ix)
        .map(|_| tokens.text(ix).to_ascii_uppercase())
        .unwrap_or_default();
    format!("{timing} {event} FOR EACH ROW")
}

/// `sql`, which creates an index or trigger (`object`), with the created
/// name qualified by `schema` when it is not already.
///
/// SQLite creates an index or trigger in the schema its name names, and in
/// `main` when the name is bare, whatever table it is on.
pub(crate) fn qualify_created(sql: &str, object: &str, schema: &str) -> String {
    let tokens = Tokens::new(sql);
    let Some(name) = tokens.after_create(object) else {
        return sql.to_string();
    };
    if name >= tokens.tokens.len() || tokens.is_symbol(name + 1, '.') {
        return sql.to_string();
    }
    let at = tokens.tokens[name].start;
    format!("{}{schema}.{}", &sql[..at], &sql[at..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_statement_gives_up_its_names_and_expressions() {
        let text = table(
            "CREATE TABLE \"line items\" (\n\
             -- the key\n\
             id INTEGER CONSTRAINT items_pk PRIMARY KEY AUTOINCREMENT,\n\
             [order] int REFERENCES orders (id) ON DELETE CASCADE,\n\
             price numeric(10, 2) DEFAULT (0.0) CHECK (price >= 0),\n\
             total AS (price * 2) STORED,\n\
             note text, -- free, with a comma\n\
             CONSTRAINT one_per_order UNIQUE (\"order\", note),\n\
             CHECK (note <> 'x, y')\n\
             ) WITHOUT ROWID",
        );
        assert!(text.without_rowid);
        let names: Vec<&str> = text.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "order", "price", "total", "note"]);
        assert!(text.column("ID").unwrap().autoincrement);
        assert_eq!(
            text.column("total").unwrap().generated.as_deref(),
            Some("price * 2")
        );
        assert_eq!(
            text.constraints,
            vec![
                ConstraintText {
                    name: Some("items_pk".into()),
                    rule: RuleText::PrimaryKey {
                        columns: vec!["id".into()]
                    }
                },
                ConstraintText {
                    name: None,
                    rule: RuleText::ForeignKey {
                        columns: vec!["order".into()]
                    }
                },
                ConstraintText {
                    name: None,
                    rule: RuleText::Check {
                        expression: "price >= 0".into()
                    }
                },
                ConstraintText {
                    name: Some("one_per_order".into()),
                    rule: RuleText::Unique {
                        columns: vec!["order".into(), "note".into()]
                    }
                },
                ConstraintText {
                    name: None,
                    rule: RuleText::Check {
                        expression: "note <> 'x, y'".into()
                    }
                },
            ]
        );
    }

    #[test]
    fn autoincrement_on_a_table_key_marks_its_column() {
        let text = table("CREATE TABLE t (id integer, PRIMARY KEY (id AUTOINCREMENT))");
        assert!(text.column("id").unwrap().autoincrement);
    }

    #[test]
    fn a_view_query_is_what_follows_as() {
        assert_eq!(
            view_query("CREATE VIEW IF NOT EXISTS main.v (a, b) AS\nSELECT 1 AS a, 2 AS b;")
                .as_deref(),
            Some("SELECT 1 AS a, 2 AS b")
        );
    }

    #[test]
    fn an_index_keeps_its_expressions_and_condition() {
        assert_eq!(
            index("CREATE UNIQUE INDEX i ON t (a DESC, lower(b), c COLLATE NOCASE) WHERE a > 1"),
            IndexText {
                keys: vec!["a".into(), "lower(b)".into(), "c COLLATE NOCASE".into()],
                predicate: Some("a > 1".into()),
            }
        );
    }

    #[test]
    fn trigger_timing_reads_the_statement() {
        assert_eq!(
            trigger_timing("CREATE TRIGGER t AFTER UPDATE OF a ON x BEGIN SELECT 1; END"),
            "AFTER UPDATE FOR EACH ROW"
        );
        assert_eq!(
            trigger_timing(
                "create temp trigger if not exists s.t instead of delete on v begin select 1; end"
            ),
            "INSTEAD OF DELETE FOR EACH ROW"
        );
        assert_eq!(
            trigger_timing("CREATE TRIGGER t INSERT ON x BEGIN SELECT 1; END"),
            "BEFORE INSERT FOR EACH ROW"
        );
    }

    #[test]
    fn created_names_are_qualified_once() {
        assert_eq!(
            qualify_created("CREATE INDEX i ON t (a)", "INDEX", "aux"),
            "CREATE INDEX aux.i ON t (a)"
        );
        assert_eq!(
            qualify_created("CREATE INDEX x.i ON t (a)", "INDEX", "aux"),
            "CREATE INDEX x.i ON t (a)"
        );
    }
}
