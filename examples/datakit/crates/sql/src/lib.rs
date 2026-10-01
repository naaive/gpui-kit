//! DataKit's understanding of SQL text.
//!
//! Everything here works on text that is being typed: it never fails on a
//! half-written statement, and it reads the PostgreSQL lexical rules that
//! decide where a statement ends — quoted strings, quoted identifiers,
//! dollar-quoted bodies, nested comments — so a `;` inside any of them is
//! not a boundary.
//!
//! - [`lex`] splits text into [`Token`]s.
//! - [`split_statements`] and [`statement_at`] find statement boundaries,
//!   which is what "execute the statement at the caret" needs.
//! - [`complete`] offers keywords, schemas, relations and columns from a
//!   [`Catalog`](datakit_catalog::Catalog) for a caret position, and
//!   [`templates`] expand abbreviations into statements.
//! - [`resolve`] tells what the name under the caret refers to, for quick
//!   documentation and Go to Declaration.
//! - [`inspect`] finds unknown names, ambiguous columns and statements that
//!   would change every row, with fixes.
//! - [`format_sql`] lays statements out; [`parameters`] and [`substitute`]
//!   fill in `$1` and `:name` placeholders.

mod analysis;
mod completion;
mod filter;
mod format;
mod inspect;
mod intention;
mod lexer;
mod parameter_info;
mod parameters;
mod read_only;
mod rename;
mod resolve;
mod statement;
pub mod templates;
#[cfg(test)]
mod test_support;

pub use completion::{Candidate, CandidateCategory, CompletionRequest, Completions, complete};
pub use filter::filtered_statement;
pub use format::{FormatStyle, format_sql};
pub use inspect::{Fix, Inspection, Problem, Severity, inspect};
pub use intention::{Intention, Refactoring, intentions};
pub use lexer::{Lexeme, Token, lex};
pub use parameter_info::{ParameterInfo, Signature, parameter_info};
pub use parameters::{Parameter, parameters, substitute};
pub use read_only::is_reading_statement;
pub use rename::{Occurrences, alias_occurrences, rename};
pub use resolve::{Resolution, Target, resolve, usages};
pub use statement::{line_at, split_lines, split_statements, statement_at};
