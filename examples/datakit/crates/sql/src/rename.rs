//! Renaming a name the statement declares itself: an alias.

use std::ops::Range;

use datakit_driver::Dialect;

use crate::{
    analysis::{identifier, references, significant, statement_tokens},
    lexer::Lexeme,
};

/// A name declared in a statement and every place the statement uses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrences {
    name: String,
    ranges: Vec<Range<usize>>,
}

impl Occurrences {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the name is written, in order; the declaration among them.
    pub fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }
}

/// The alias at byte `offset` of `text` — where it is declared or where it
/// qualifies a column — and every use of it in its statement.
pub fn alias_occurrences(text: &str, offset: usize, dialect: &dyn Dialect) -> Option<Occurrences> {
    let (_, tokens) = statement_tokens(text, offset)?;
    let references = references(text, &tokens, dialect);
    let significant = significant(&tokens);
    let at = significant.iter().position(|token| {
        let range = token.range();
        range.start <= offset && offset <= range.end && token.lexeme() != Lexeme::Whitespace
    })?;
    let name = identifier(text, significant[at], dialect)?;
    let reference = references
        .iter()
        .find(|reference| reference.alias.as_deref() == Some(name.as_str()))?;
    let declaration = reference.alias_range.clone()?;
    let mut ranges = vec![declaration.clone()];
    for (ix, token) in significant.iter().enumerate() {
        if token.range() == declaration {
            continue;
        }
        // A use qualifies something: `alias.column`.
        let qualifies = significant
            .get(ix + 1)
            .is_some_and(|next| next.lexeme() == Lexeme::Period);
        let qualified = ix > 0 && significant[ix - 1].lexeme() == Lexeme::Period;
        if qualifies
            && !qualified
            && identifier(text, token, dialect).as_deref() == Some(name.as_str())
        {
            ranges.push(token.range());
        }
    }
    ranges.sort_by_key(|range| range.start);
    // The caret must be on one of them.
    ranges
        .iter()
        .any(|range| range.start <= offset && offset <= range.end)
        .then_some(Occurrences { name, ranges })
}

/// `text` with every range of `occurrences` replaced by `new_name`.
pub fn rename(text: &str, occurrences: &Occurrences, new_name: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut position = 0;
    for range in occurrences.ranges() {
        result.push_str(&text[position..range.start]);
        result.push_str(new_name);
        position = range.end;
    }
    result.push_str(&text[position..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Plain, caret};

    #[test]
    fn an_alias_is_found_from_its_declaration_or_a_use() {
        let sql = "select o.id, o.total from orders o join customers c on c.id = o.customer_id";
        for at in ["select o|.id", "from orders o|", "= o|.customer_id"] {
            let (marked, _) = caret(at);
            let offset = sql.find(&marked).unwrap() + at.find('|').unwrap();
            let found = alias_occurrences(sql, offset, &Plain).expect(at);
            assert_eq!(found.name(), "o");
            assert_eq!(found.ranges().len(), 4, "{at}");
        }
    }

    #[test]
    fn renaming_replaces_every_use_and_nothing_else() {
        let (text, offset) =
            caret("select o.id, total from orders o| where o.total > 1 and so.x = 1");
        let found = alias_occurrences(&text, offset, &Plain).unwrap();
        assert_eq!(
            rename(&text, &found, "ord"),
            "select ord.id, total from orders ord where ord.total > 1 and so.x = 1"
        );
    }

    #[test]
    fn a_table_name_is_not_an_alias() {
        let (text, offset) = caret("select * from ord|ers o");
        assert!(alias_occurrences(&text, offset, &Plain).is_none());
    }
}
