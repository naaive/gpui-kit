//! Completion for a caret in SQL text.
//!
//! The engine reads the statement around the caret the way a person does:
//! after `FROM` or `JOIN` it offers relations, after `orders.` it offers the
//! columns of `orders` (or of whatever `o.` is an alias for), and anywhere
//! else it offers the columns of the relations the statement already names,
//! then keywords and functions. After `JOIN … ON` it first offers the
//! conditions the foreign keys between the joined relations imply. It never
//! needs the statement to parse.

use std::{collections::HashSet, ops::Range, sync::Arc};

use datakit_catalog::{Catalog, ForeignKey, Relation, RelationType};
use datakit_driver::Dialect;

use crate::{
    analysis::{Reference, identifier, references},
    lexer::{Lexeme, Token, lex},
    statement::statement_at,
};

/// What a [`Candidate`] stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CandidateCategory {
    Keyword,
    Function,
    Schema,
    Table,
    View,
    Column,
    /// A name the statement itself introduced with `AS`.
    Alias,
    /// A join condition a foreign key implies.
    JoinCondition,
}

/// One suggestion.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    label: Arc<str>,
    insert_text: String,
    category: CandidateCategory,
    detail: Option<Arc<str>>,
}

impl Candidate {
    fn new(label: impl Into<Arc<str>>, insert_text: String, category: CandidateCategory) -> Self {
        Self {
            label: label.into(),
            insert_text,
            category,
            detail: None,
        }
    }

    fn with_detail(mut self, detail: impl Into<Arc<str>>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The name as the list shows it.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The text that replaces [`Completions::replace_range`], quoted as the
    /// dialect needs.
    pub fn insert_text(&self) -> &str {
        &self.insert_text
    }

    pub fn category(&self) -> CandidateCategory {
        self.category
    }

    /// Secondary text: a column's type, a relation's schema.
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

/// The input to [`complete`].
pub struct CompletionRequest<'a> {
    text: &'a str,
    offset: usize,
    catalog: &'a Catalog,
    dialect: &'a dyn Dialect,
}

impl<'a> CompletionRequest<'a> {
    /// Complete at byte `offset` of `text`.
    pub fn new(
        text: &'a str,
        offset: usize,
        catalog: &'a Catalog,
        dialect: &'a dyn Dialect,
    ) -> Self {
        Self {
            text,
            offset: offset.min(text.len()),
            catalog,
            dialect,
        }
    }
}

/// The result of [`complete`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completions {
    replace_range: Range<usize>,
    candidates: Vec<Candidate>,
    missing_schemas: Vec<Arc<str>>,
}

impl Completions {
    /// The bytes of the text a chosen candidate replaces: the partial word
    /// before the caret.
    pub fn replace_range(&self) -> Range<usize> {
        self.replace_range.clone()
    }

    /// Candidates, best first.
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// Schemas whose relations the answer needed but the catalog has not
    /// loaded. The caller can introspect them and ask again.
    pub fn missing_schemas(&self) -> &[Arc<str>] {
        &self.missing_schemas
    }
}

/// Suggest what can be typed at the request's caret.
pub fn complete(request: &CompletionRequest) -> Completions {
    let CompletionRequest {
        text,
        offset,
        catalog,
        dialect,
    } = *request;

    let statement = statement_at(text, offset)
        .filter(|range| range.start <= offset)
        .map(|range| range.start..range.end.max(offset))
        .unwrap_or(offset..offset);
    let tokens: Vec<Token> = lex(&text[statement.clone()])
        .into_iter()
        .map(|token| token.offset_by(statement.start))
        .collect();

    // The caret must be at the end of a word (or between tokens), never
    // inside a string or a comment.
    let current = tokens
        .iter()
        .position(|token| token.range().start < offset && offset <= token.range().end);
    let (prefix_start, before_ix) = match current.map(|ix| (ix, &tokens[ix])) {
        Some((ix, token)) => match token.lexeme() {
            Lexeme::Word => (token.range().start, ix),
            Lexeme::Whitespace | Lexeme::Comma | Lexeme::OpenParen | Lexeme::Operator => {
                (offset, ix + 1)
            }
            Lexeme::Period | Lexeme::CloseParen => (offset, ix + 1),
            _ => return Completions::default(),
        },
        None => (offset, tokens.len()),
    };
    let prefix = &text[prefix_start..offset];

    let context = Context::read(text, &tokens[..before_ix], dialect);
    let references = references(text, &tokens, dialect);

    let mut engine = Engine {
        catalog,
        dialect,
        prefix,
        candidates: Vec::new(),
        missing_schemas: Vec::new(),
        seen: HashSet::new(),
    };

    match context {
        Context::Qualified(qualifier) => engine.qualified(&qualifier, &references),
        Context::Relation => {
            engine.relations();
            engine.schemas();
        }
        Context::JoinCondition => {
            engine.join_conditions(&references, prefix_start);
            engine.referenced_columns(&references);
            engine.aliases(&references);
            engine.functions();
            engine.keywords();
        }
        Context::Expression => {
            engine.referenced_columns(&references);
            engine.aliases(&references);
            engine.functions();
            engine.keywords();
        }
        Context::Clause => {
            engine.keywords();
            engine.referenced_columns(&references);
        }
    }

    let mut candidates = engine.candidates;
    let missing_schemas = engine.missing_schemas;
    // Stable: candidates whose label starts with the prefix come first, in
    // the order the context put them; the rest only contain it.
    candidates.sort_by_key(|candidate| !starts_with_ignore_case(candidate.label(), prefix));
    Completions {
        replace_range: prefix_start..offset,
        candidates,
        missing_schemas,
    }
}

/// Where the caret stands in the statement.
#[derive(Debug, PartialEq)]
enum Context {
    /// Right after `name.`: what `name` contains.
    Qualified(String),
    /// Where a relation name goes: after `FROM`, `JOIN`, `INTO`, `UPDATE`.
    Relation,
    /// Right after `ON` in a join.
    JoinCondition,
    /// Inside an expression: a select list, a condition.
    Expression,
    /// Between clauses, where the next keyword goes.
    Clause,
}

const RELATION_KEYWORDS: &[&str] = &["from", "join", "into", "update", "table", "truncate"];
const EXPRESSION_KEYWORDS: &[&str] = &[
    "select",
    "where",
    "on",
    "and",
    "or",
    "not",
    "by",
    "having",
    "set",
    "values",
    "returning",
    "when",
    "then",
    "else",
    "using",
    "distinct",
];

impl Context {
    fn read(text: &str, before: &[Token], dialect: &dyn Dialect) -> Self {
        let mut significant = before
            .iter()
            .rev()
            .filter(|token| token.lexeme().is_significant());
        let Some(last) = significant.next() else {
            return Context::Clause;
        };
        if last.lexeme() == Lexeme::Period {
            if let Some(qualifier) = significant
                .next()
                .and_then(|t| identifier(text, t, dialect))
            {
                return Context::Qualified(qualifier);
            }
            return Context::Expression;
        }
        if RELATION_KEYWORDS
            .iter()
            .any(|keyword| last.is_keyword(text, keyword))
        {
            return Context::Relation;
        }
        if last.lexeme() == Lexeme::Comma {
            // A comma continues whatever list it is in; find the clause.
            let clause = before
                .iter()
                .rev()
                .filter(|token| token.lexeme() == Lexeme::Word)
                .find(|token| {
                    RELATION_KEYWORDS
                        .iter()
                        .chain(EXPRESSION_KEYWORDS)
                        .any(|keyword| token.is_keyword(text, keyword))
                });
            return match clause {
                Some(token) if token.is_keyword(text, "from") => Context::Relation,
                _ => Context::Expression,
            };
        }
        if last.is_keyword(text, "on") {
            return Context::JoinCondition;
        }
        if matches!(
            last.lexeme(),
            Lexeme::OpenParen | Lexeme::Operator | Lexeme::Comma
        ) || EXPRESSION_KEYWORDS
            .iter()
            .any(|keyword| last.is_keyword(text, keyword))
        {
            return Context::Expression;
        }
        Context::Clause
    }
}

struct Engine<'a> {
    catalog: &'a Catalog,
    dialect: &'a dyn Dialect,
    prefix: &'a str,
    candidates: Vec<Candidate>,
    missing_schemas: Vec<Arc<str>>,
    seen: HashSet<(CandidateCategory, String)>,
}

impl<'a> Engine<'a> {
    fn push(&mut self, candidate: Candidate) {
        if !matches_prefix(candidate.label(), self.prefix) {
            return;
        }
        if self
            .seen
            .insert((candidate.category(), candidate.label().to_string()))
        {
            self.candidates.push(candidate);
        }
    }

    fn quote(&self, name: &str) -> String {
        self.dialect.quote_identifier(name)
    }

    fn qualified(&mut self, qualifier: &str, references: &[Reference]) {
        // An alias or relation of this statement wins over a schema of the
        // same name, as it does when the database resolves the statement.
        if let Some(reference) = references
            .iter()
            .find(|reference| reference.visible_name() == qualifier)
        {
            if let Some(relation) = self.resolve(reference) {
                self.columns(relation, None);
            }
            return;
        }
        let catalog = self.catalog;
        match catalog.schema(qualifier) {
            Some(schema) => match schema.relations() {
                Some(relations) => {
                    for relation in relations {
                        self.push(relation_candidate(self, relation, None));
                    }
                }
                None => self.missing_schemas.push(schema.name()),
            },
            None => {
                // A relation named directly, without being in FROM yet.
                if let Some(relation) = catalog.resolve_relation(None, qualifier) {
                    self.columns(relation, None);
                }
            }
        }
    }

    fn resolve(&mut self, reference: &Reference) -> Option<&'a Relation> {
        let catalog = self.catalog;
        if let Some(schema) = &reference.schema
            && let Some(found) = catalog.schema(schema)
            && found.relations().is_none()
        {
            self.missing_schemas.push(found.name());
        }
        catalog.resolve_relation(reference.schema.as_deref(), &reference.relation)
    }

    fn columns(&mut self, relation: &Relation, owner: Option<&str>) {
        for column in relation.columns() {
            let detail = match owner {
                Some(owner) => format!("{owner} · {}", column.data_type()),
                None => column.data_type().to_string(),
            };
            let candidate = Candidate::new(
                column.name(),
                self.quote(&column.name()),
                CandidateCategory::Column,
            )
            .with_detail(detail);
            self.push(candidate);
        }
    }

    /// The conditions joining the relation named last before `caret` to the
    /// relations named before it, one per foreign key between them in
    /// either direction.
    fn join_conditions(&mut self, references: &[Reference], caret: usize) {
        let Some(joined_ix) = references
            .iter()
            .rposition(|reference| reference.relation_range.end <= caret)
        else {
            return;
        };
        let joined = &references[joined_ix];
        let Some(joined_relation) = self.resolve(joined) else {
            return;
        };
        for earlier in &references[..joined_ix] {
            let Some(earlier_relation) = self.resolve(earlier) else {
                continue;
            };
            for (from, from_relation, to, to_relation) in [
                (joined, joined_relation, earlier, earlier_relation),
                (earlier, earlier_relation, joined, joined_relation),
            ] {
                for (_, key) in from_relation.foreign_keys() {
                    if !refers_to(key, to, to_relation) {
                        continue;
                    }
                    let pairs: Vec<(String, String)> = key
                        .columns()
                        .iter()
                        .zip(key.referenced_columns())
                        .map(|(column, referenced)| {
                            (
                                format!("{}.{}", from.visible_name(), column),
                                format!("{}.{}", to.visible_name(), referenced),
                            )
                        })
                        .collect();
                    let label = pairs
                        .iter()
                        .map(|(left, right)| format!("{left} = {right}"))
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    let insert_text = key
                        .columns()
                        .iter()
                        .zip(key.referenced_columns())
                        .map(|(column, referenced)| {
                            format!(
                                "{}.{} = {}.{}",
                                self.quote(from.visible_name()),
                                self.quote(column),
                                self.quote(to.visible_name()),
                                self.quote(referenced)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    self.push(Candidate::new(
                        label,
                        insert_text,
                        CandidateCategory::JoinCondition,
                    ));
                }
            }
        }
    }

    fn referenced_columns(&mut self, references: &[Reference]) {
        for reference in references {
            if let Some(relation) = self.resolve(reference) {
                self.columns(relation, Some(reference.visible_name()));
            }
        }
    }

    fn aliases(&mut self, references: &[Reference]) {
        for reference in references {
            if let Some(alias) = &reference.alias {
                let candidate =
                    Candidate::new(alias.as_str(), self.quote(alias), CandidateCategory::Alias)
                        .with_detail(reference.relation.as_str());
                self.push(candidate);
            }
        }
    }

    fn relations(&mut self) {
        let catalog = self.catalog;
        let search_path = catalog.search_path();
        for schema_name in search_path {
            let Some(schema) = catalog.schema(schema_name) else {
                continue;
            };
            match schema.relations() {
                Some(relations) => {
                    for relation in relations {
                        self.push(relation_candidate(self, relation, None));
                    }
                }
                None => self.missing_schemas.push(schema.name()),
            }
        }
        // Relations outside the search path need their schema to be found.
        for (schema, relation) in catalog.relations() {
            if search_path.contains(&schema.name()) {
                continue;
            }
            self.push(relation_candidate(self, relation, Some(&schema.name())));
        }
    }

    fn schemas(&mut self) {
        let catalog = self.catalog;
        for schema in catalog.schemas() {
            let candidate = Candidate::new(
                schema.name(),
                self.quote(&schema.name()),
                CandidateCategory::Schema,
            );
            self.push(candidate);
        }
    }

    fn functions(&mut self) {
        for function in self.dialect.functions() {
            self.push(Candidate::new(
                *function,
                format!("{function}("),
                CandidateCategory::Function,
            ));
        }
    }

    fn keywords(&mut self) {
        // Keywords are inserted in the case the person is typing in.
        let lower = !self.prefix.is_empty() && self.prefix.chars().all(|c| !c.is_uppercase());
        for keyword in self.dialect.keywords() {
            let insert = if lower {
                keyword.to_lowercase()
            } else {
                keyword.to_string()
            };
            self.push(Candidate::new(*keyword, insert, CandidateCategory::Keyword));
        }
    }
}

/// Whether `key` points at `relation`, which `reference` names.
fn refers_to(key: &ForeignKey, reference: &Reference, relation: &Relation) -> bool {
    *key.referenced_relation() == *relation.name()
        && reference
            .schema
            .as_deref()
            .is_none_or(|schema| schema == key.referenced_schema())
}

fn relation_candidate(engine: &Engine, relation: &Relation, schema: Option<&str>) -> Candidate {
    let category = match relation.relation_type() {
        RelationType::View | RelationType::MaterializedView => CandidateCategory::View,
        _ => CandidateCategory::Table,
    };
    let name = relation.name();
    let insert = match schema {
        Some(schema) => engine.dialect.qualified_name(schema, &name),
        None => engine.quote(&name),
    };
    let candidate = Candidate::new(name, insert, category);
    match schema {
        Some(schema) => candidate.with_detail(schema),
        None => candidate,
    }
}

fn starts_with_ignore_case(label: &str, prefix: &str) -> bool {
    label
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

fn matches_prefix(label: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || starts_with_ignore_case(label, prefix)
        || label.to_lowercase().contains(&prefix.to_lowercase())
}

#[cfg(test)]
mod tests {
    use datakit_catalog::{Column, Constraint, ConstraintRule, Schema};

    use super::*;

    struct Postgres;

    impl Dialect for Postgres {
        fn reserved_words(&self) -> &'static [&'static str] {
            &[
                "SELECT", "FROM", "WHERE", "AS", "ON", "JOIN", "USER", "ORDER",
            ]
        }

        fn keywords(&self) -> &'static [&'static str] {
            &["SELECT", "FROM", "WHERE", "ORDER BY"]
        }

        fn functions(&self) -> &'static [&'static str] {
            &["count", "coalesce"]
        }
    }

    fn catalog() -> Catalog {
        Catalog::new("shop")
            .with_schemas([Schema::new("public"), Schema::new("audit")])
            .with_relations(
                "public",
                [
                    Relation::new("orders", RelationType::Table)
                        .with_columns([
                            Column::new("id", "integer"),
                            Column::new("customer_id", "integer"),
                            Column::new("total", "numeric"),
                        ])
                        .with_constraints([Constraint::new(
                            "orders_customer_id_fkey",
                            ConstraintRule::ForeignKey(ForeignKey::new(
                                ["customer_id"],
                                "public",
                                "customers",
                                ["id"],
                            )),
                        )]),
                    Relation::new("customers", RelationType::Table)
                        .with_columns([Column::new("id", "integer"), Column::new("Name", "text")]),
                    Relation::new("order_totals", RelationType::View),
                ],
            )
            .with_search_path(["public".into()])
    }

    fn labels(text: &str) -> Vec<String> {
        let (text, offset) = caret(text);
        let catalog = catalog();
        complete(&CompletionRequest::new(&text, offset, &catalog, &Postgres))
            .candidates()
            .iter()
            .map(|candidate| candidate.label().to_string())
            .collect()
    }

    /// The text with `|` removed, and where it was.
    fn caret(text: &str) -> (String, usize) {
        let offset = text.find('|').expect("a caret");
        (text.replacen('|', "", 1), offset)
    }

    #[test]
    fn after_from_it_offers_relations_then_schemas() {
        let found = labels("select * from |");
        assert_eq!(
            found,
            vec!["customers", "order_totals", "orders", "public", "audit"]
        );
    }

    #[test]
    fn a_prefix_filters_and_ranks_starts_before_contains() {
        assert_eq!(labels("select * from ord|"), vec!["order_totals", "orders"]);
        let found = labels("select * from tom|");
        assert_eq!(found, vec!["customers"]);
    }

    #[test]
    fn an_alias_qualifier_offers_that_relations_columns() {
        let found = labels("select o.| from orders o");
        assert_eq!(found, vec!["id", "customer_id", "total"]);
        let found = labels("select c.n| from public.customers as c");
        assert_eq!(found, vec!["Name"]);
    }

    #[test]
    fn a_schema_qualifier_offers_its_relations_or_asks_to_load_them() {
        assert_eq!(
            labels("select * from public.o|"),
            vec!["order_totals", "orders", "customers"],
            "names containing the prefix follow names starting with it"
        );

        let (text, offset) = caret("select * from audit.|");
        let catalog = catalog();
        let completions = complete(&CompletionRequest::new(&text, offset, &catalog, &Postgres));
        assert!(completions.candidates().is_empty());
        assert_eq!(completions.missing_schemas(), &["audit".into()]);
    }

    #[test]
    fn expressions_offer_columns_of_the_named_relations_first() {
        let found = labels("select | from orders o join customers c on c.id = o.customer_id");
        assert_eq!(
            &found[..5],
            &["id", "customer_id", "total", "Name", "o"],
            "columns, then aliases"
        );
        assert!(found.contains(&"count".to_string()));
        assert!(found.contains(&"SELECT".to_string()));
    }

    #[test]
    fn after_on_the_foreign_keys_suggest_the_join_condition() {
        let found = labels("select * from orders o join customers c on |");
        assert_eq!(found[0], "o.customer_id = c.id");
        // The key is found from either side of the join.
        let found = labels("select * from customers c join orders o on |");
        assert_eq!(found[0], "o.customer_id = c.id");
    }

    #[test]
    fn a_comma_in_the_from_list_is_still_a_relation_position() {
        let found = labels("select * from orders, cust|");
        assert_eq!(found, vec!["customers"]);
    }

    #[test]
    fn identifiers_that_need_quotes_are_inserted_quoted() {
        let (text, offset) = caret("select c.| from customers c");
        let catalog = catalog();
        let completions = complete(&CompletionRequest::new(&text, offset, &catalog, &Postgres));
        let name = completions
            .candidates()
            .iter()
            .find(|candidate| candidate.label() == "Name")
            .unwrap();
        assert_eq!(name.insert_text(), "\"Name\"");
    }

    #[test]
    fn the_replace_range_is_the_word_before_the_caret() {
        let (text, offset) = caret("select * from ord|");
        let catalog = catalog();
        let completions = complete(&CompletionRequest::new(&text, offset, &catalog, &Postgres));
        assert_eq!(&text[completions.replace_range()], "ord");
    }

    #[test]
    fn keywords_follow_the_case_being_typed() {
        let (text, offset) = caret("sel|");
        let catalog = catalog();
        let completions = complete(&CompletionRequest::new(&text, offset, &catalog, &Postgres));
        let select = &completions.candidates()[0];
        assert_eq!(select.label(), "SELECT");
        assert_eq!(select.insert_text(), "select");
    }

    #[test]
    fn nothing_is_offered_inside_a_string_or_comment() {
        assert!(labels("select 'ord|'").is_empty());
        assert!(labels("select 1 -- ord|").is_empty());
    }

    #[test]
    fn the_statement_around_the_caret_is_the_only_one_read() {
        let found = labels("select * from customers c;\nselect c.| from orders c");
        assert_eq!(found, vec!["id", "customer_id", "total"]);
    }
}
