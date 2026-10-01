use std::ops::Range;

use crate::lexer::{Lexeme, Token, lex};

/// The statements in `text`, as byte ranges that start at a statement's first
/// meaningful token and end after its last, without the `;`.
///
/// A `;` ends a statement, except inside the body of a `BEGIN ATOMIC … END`
/// function or of a routine or trigger whose body is a `BEGIN … END` block
/// (MySQL, SQL Server). A line holding only `GO` ends a statement too, as
/// SQL Server's tools read it. So does a line that starts a new statement — `SELECT`,
/// `INSERT`, `CREATE` and the like at the start of a line, outside any
/// parentheses — when the statement before it cannot continue that way:
/// people leave the `;` off the last line they typed, and DataGrip reads
/// such a console the same way. `INSERT … SELECT`, `WITH … SELECT`,
/// `CREATE … AS SELECT`, `UNION` and lines that continue an expression stay
/// one statement. Text that is only whitespace and comments is not a
/// statement.
pub fn split_statements(text: &str) -> Vec<Range<usize>> {
    spans(text, &lex(text))
        .into_iter()
        .map(|span| span.range)
        .collect()
}

/// The statement to run for a caret at `offset`, as a byte range.
///
/// A caret inside a statement, or after its `;` on the same line, picks that
/// statement. A caret on a line of its own between statements picks the next
/// one, or the last one when there is no next.
pub fn statement_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let spans = spans(text, &lex(text));
    let offset = offset.min(text.len());
    let previous = spans.iter().rposition(|span| span.range.start <= offset);
    if let Some(ix) = previous {
        let span = &spans[ix];
        if offset <= span.terminated_end {
            return Some(span.range.clone());
        }
        let after = &text[span.terminated_end..offset];
        if !after.contains('\n') {
            return Some(span.range.clone());
        }
    }
    let next = previous.map_or(0, |ix| ix + 1);
    spans
        .get(next)
        .or_else(|| previous.map(|ix| &spans[ix]))
        .map(|span| span.range.clone())
}

struct Span {
    range: Range<usize>,
    /// The end including the `;`, when there is one.
    terminated_end: usize,
}

/// Words that begin a statement when they start a line.
const STATEMENT_STARTS: &[&str] = &[
    "select", "insert", "update", "delete", "create", "alter", "drop", "truncate", "grant",
    "revoke", "commit", "rollback", "explain", "vacuum", "show", "copy", "call", "merge", "with",
];

/// Statements that contain one query of their own: `INSERT … SELECT`,
/// `WITH … SELECT`, `CREATE TABLE … AS SELECT`, `EXPLAIN SELECT`.
const QUERY_HOLDERS: &[&str] = &[
    "insert", "create", "explain", "with", "copy", "declare", "prepare",
];

/// The words that can begin that inner query.
const QUERY_HEADS: &[&str] = &["select", "with", "values", "insert", "update", "delete"];

/// Words after which a statement keyword on the next line continues the
/// statement instead of starting one.
const CONTINUING_WORDS: &[&str] = &[
    "union",
    "except",
    "intersect",
    "all",
    "distinct",
    "as",
    "then",
    "else",
    "do",
    "instead",
    "also",
    "before",
    "after",
    "or",
    "and",
    "on",
    "grant",
    "revoke",
    "for",
    "explain",
    "analyze",
    "verbose",
    "not",
    "in",
    "exists",
    "lateral",
    "into",
    "of",
];

/// What the splitter knows about the statement it is in.
#[derive(Default)]
struct Statement<'a> {
    start: Option<usize>,
    end: usize,
    first_word: Option<String>,
    /// Whether the statement may still take its one inner query.
    takes_query: bool,
    paren_depth: usize,
    /// How deep the statement is in `BEGIN … END` bodies.
    atomic_depth: usize,
    case_depth: usize,
    /// Whether the statement creates a routine or trigger, whose `BEGIN`
    /// opens a body.
    routine: bool,
    previous: Option<&'a Token>,
    previous_word: Option<&'a Token>,
}

fn spans(text: &str, tokens: &[Token]) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut statement = Statement::default();

    for token in tokens {
        let lexeme = token.lexeme();
        if !lexeme.is_significant() {
            continue;
        }
        if lexeme == Lexeme::Semicolon && statement.atomic_depth == 0 {
            if let Some(start) = statement.start {
                spans.push(Span {
                    range: start..statement.end,
                    terminated_end: token.range().end,
                });
            }
            statement = Statement::default();
            continue;
        }
        if lexeme == Lexeme::Word && is_batch_separator(text, token) {
            if let Some(start) = statement.start {
                spans.push(Span {
                    range: start..statement.end,
                    terminated_end: token.range().end,
                });
            }
            statement = Statement::default();
            continue;
        }
        if lexeme == Lexeme::Word && starts_new_statement(text, token, &mut statement) {
            if let Some(start) = statement.start {
                spans.push(Span {
                    range: start..statement.end,
                    terminated_end: statement.end,
                });
            }
            statement = Statement::default();
        }
        match lexeme {
            Lexeme::OpenParen => statement.paren_depth += 1,
            Lexeme::CloseParen => statement.paren_depth = statement.paren_depth.saturating_sub(1),
            Lexeme::Word => {
                let after_end = statement
                    .previous_word
                    .is_some_and(|word| word.is_keyword(text, "end"));
                if statement
                    .first_word
                    .as_deref()
                    .is_some_and(|word| matches!(word, "create" | "alter"))
                    && ["function", "procedure", "trigger", "event"]
                        .iter()
                        .any(|object| token.is_keyword(text, object))
                {
                    statement.routine = true;
                }
                if token.is_keyword(text, "case") {
                    // `END CASE` closes a CASE statement rather than opening one.
                    if !after_end {
                        statement.case_depth += 1;
                    }
                } else if token.is_keyword(text, "begin") && statement.routine {
                    statement.atomic_depth += 1;
                } else if token.is_keyword(text, "atomic")
                    && !statement.routine
                    && statement
                        .previous_word
                        .is_some_and(|word| word.is_keyword(text, "begin"))
                {
                    statement.atomic_depth += 1;
                } else if after_end
                    && ["if", "loop", "while", "repeat"]
                        .iter()
                        .any(|block| token.is_keyword(text, block))
                {
                    // `END IF` closes a block that was never counted.
                    statement.atomic_depth += 1;
                } else if token.is_keyword(text, "end") {
                    if statement.case_depth > 0 {
                        statement.case_depth -= 1;
                    } else {
                        statement.atomic_depth = statement.atomic_depth.saturating_sub(1);
                    }
                }
                if statement.first_word.is_none() {
                    let word = token.text(text).to_ascii_lowercase();
                    statement.takes_query = QUERY_HOLDERS.contains(&word.as_str());
                    statement.first_word = Some(word);
                }
                statement.previous_word = Some(token);
            }
            _ => {}
        }
        statement.start.get_or_insert(token.range().start);
        statement.end = token.range().end;
        statement.previous = Some(token);
    }
    if let Some(start) = statement.start {
        spans.push(Span {
            range: start..statement.end,
            terminated_end: statement.end,
        });
    }
    spans
}

/// Whether `token` is SQL Server's `GO`: the only word on its line.
fn is_batch_separator(text: &str, token: &Token) -> bool {
    if !token.is_keyword(text, "go") {
        return false;
    }
    let line_start = text[..token.range().start]
        .rfind('\n')
        .map_or(0, |ix| ix + 1);
    let line_end = text[token.range().end..]
        .find('\n')
        .map_or(text.len(), |ix| token.range().end + ix);
    text[line_start..token.range().start].trim().is_empty()
        && text[token.range().end..line_end].trim().is_empty()
}

/// Whether `token`, a word, ends `statement` and begins another.
fn starts_new_statement(text: &str, token: &Token, statement: &mut Statement) -> bool {
    let Some(previous) = statement.previous else {
        return false;
    };
    if statement.paren_depth > 0 || statement.atomic_depth > 0 || statement.case_depth > 0 {
        return false;
    }
    let word = token.text(text).to_ascii_lowercase();
    // The statement's own query, wherever it starts, uses up the one it may
    // hold; a second one on a new line is a new statement.
    let is_inner_query = statement.takes_query && QUERY_HEADS.contains(&word.as_str());
    if is_inner_query {
        statement.takes_query = false;
    }
    let at_line_start = text[statement.end..token.range().start].contains('\n');
    if !at_line_start || is_inner_query || !STATEMENT_STARTS.contains(&word.as_str()) {
        return false;
    }
    let continues = match previous.lexeme() {
        Lexeme::OpenParen | Lexeme::Comma | Lexeme::Operator | Lexeme::Period => true,
        Lexeme::Word => CONTINUING_WORDS
            .iter()
            .any(|continuing| previous.is_keyword(text, continuing)),
        _ => false,
    };
    !continues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statements(text: &str) -> Vec<&str> {
        split_statements(text)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    #[test]
    fn routine_bodies_keep_their_semicolons() {
        let mysql = "create procedure p()\nbegin\n  declare x int;\n  if x > 1 then\n    \
                     set x = 2;\n  end if;\n  case x when 1 then select 1; end case;\nend;\n\
                     select 2;";
        assert_eq!(statements(mysql).len(), 2);
        assert!(statements(mysql)[0].ends_with("end"));
        let tsql =
            "create procedure p as\nbegin\n  select 1;\n  select 2;\nend\nGO\nselect 3\ngo\n";
        assert_eq!(
            statements(tsql),
            vec![
                "create procedure p as\nbegin\n  select 1;\n  select 2;\nend",
                "select 3"
            ]
        );
        assert_eq!(
            statements("begin;\nupdate t set a = 1;\ncommit;"),
            vec!["begin", "update t set a = 1", "commit"],
            "a transaction's BEGIN is not a body"
        );
    }

    #[test]
    fn statements_split_at_semicolons_outside_quotes_and_comments() {
        let text = "select 'a;b';\n-- one; two\nselect $$;$$ /* ; */ ;  \n\nselect 3";
        assert_eq!(
            statements(text),
            vec!["select 'a;b'", "select $$;$$", "select 3"]
        );
    }

    #[test]
    fn a_statement_keyword_starting_a_line_starts_a_statement() {
        assert_eq!(
            statements("select c.co\nselect pg_sleep(30);"),
            vec!["select c.co", "select pg_sleep(30)"]
        );
        assert_eq!(
            statements("select 1 select 2"),
            vec!["select 1 select 2"],
            "only at the start of a line"
        );
        assert_eq!(
            statements("update t\nset a = 1\ndelete from t"),
            vec!["update t\nset a = 1", "delete from t"]
        );
    }

    #[test]
    fn statements_that_hold_a_query_keep_it() {
        assert_eq!(
            statements("insert into t\nselect * from s"),
            vec!["insert into t\nselect * from s"]
        );
        assert_eq!(
            statements("create table t as\nselect 1"),
            vec!["create table t as\nselect 1"]
        );
        assert_eq!(
            statements("with x as (\n  select 1\n)\nselect * from x\nselect 2"),
            vec!["with x as (\n  select 1\n)\nselect * from x", "select 2"]
        );
        assert_eq!(
            statements("insert into t select 1\nselect 2"),
            vec!["insert into t select 1", "select 2"]
        );
    }

    #[test]
    fn continuing_lines_stay_in_their_statement() {
        assert_eq!(
            statements("select 1\nunion all\nselect 2"),
            vec!["select 1\nunion all\nselect 2"]
        );
        assert_eq!(
            statements("insert into t values (1) on conflict (id) do\nupdate set a = 1"),
            vec!["insert into t values (1) on conflict (id) do\nupdate set a = 1"]
        );
        assert_eq!(
            statements("select (\nselect 1)"),
            vec!["select (\nselect 1)"],
            "inside parentheses"
        );
        assert_eq!(
            statements("create trigger t after insert or\nupdate on x execute function f()"),
            vec!["create trigger t after insert or\nupdate on x execute function f()"]
        );
    }

    #[test]
    fn comment_only_text_is_not_a_statement() {
        assert!(statements("-- nothing\n/* here */ ;;").is_empty());
    }

    #[test]
    fn begin_atomic_bodies_stay_whole() {
        let text = "create function f() returns int language sql begin atomic \
                    select case when true then 1 end; select 2; end; select 3";
        let found = statements(text);
        assert_eq!(found.len(), 2);
        assert!(found[0].ends_with("select 2; end"));
        assert_eq!(found[1], "select 3");
    }

    #[test]
    fn the_caret_picks_the_statement_it_is_in_or_just_after() {
        let text = "select 1;   \nselect 2;\n\nselect 3;";
        let at = |offset| statement_at(text, offset).map(|range| &text[range]);
        assert_eq!(at(3), Some("select 1"));
        assert_eq!(at(9), Some("select 1"));
        assert_eq!(at(11), Some("select 1"), "after the `;` on the same line");
        assert_eq!(at(13), Some("select 2"));
        assert_eq!(at(23), Some("select 3"), "a blank line picks the next");
        assert_eq!(at(text.len()), Some("select 3"));
    }

    #[test]
    fn a_caret_before_everything_picks_the_first_statement() {
        let text = "\n\n  select 1; select 2";
        assert_eq!(
            statement_at(text, 0).map(|range| &text[range]),
            Some("select 1")
        );
        assert_eq!(statement_at("", 0), None);
    }
}
