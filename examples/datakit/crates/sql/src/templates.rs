//! Live templates: short abbreviations completion expands into whole
//! statements, the way `sel` becomes `SELECT * FROM`.

/// Marks where the caret goes after a template is inserted. An invisible
/// character the console removes right after inserting the template.
pub const CARET: char = '\u{2063}';

/// One template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    abbreviation: &'static str,
    description: &'static str,
    body: &'static str,
}

impl Template {
    pub fn abbreviation(&self) -> &'static str {
        self.abbreviation
    }

    /// What the template writes, in English; the console shows it as the
    /// completion's detail.
    pub fn description(&self) -> &'static str {
        self.description
    }

    /// The text to insert, with [`CARET`] where the caret goes.
    pub fn body(&self) -> String {
        self.body.replace('|', &CARET.to_string())
    }
}

const fn template(
    abbreviation: &'static str,
    description: &'static str,
    body: &'static str,
) -> Template {
    Template {
        abbreviation,
        description,
        body,
    }
}

/// Every template; `|` in a body marks the caret.
pub const TEMPLATES: &[Template] = &[
    template("sel", "SELECT * FROM", "SELECT * FROM |"),
    template("selw", "SELECT * FROM … WHERE", "SELECT * FROM | WHERE "),
    template("selc", "SELECT count(*) FROM", "SELECT count(*) FROM |"),
    template("seld", "SELECT DISTINCT", "SELECT DISTINCT | FROM "),
    template("ins", "INSERT INTO … VALUES", "INSERT INTO | () VALUES ()"),
    template("upd", "UPDATE … SET … WHERE", "UPDATE | SET  WHERE "),
    template("del", "DELETE FROM … WHERE", "DELETE FROM | WHERE "),
    template("lj", "LEFT JOIN … ON", "LEFT JOIN | ON "),
    template("ij", "INNER JOIN … ON", "INNER JOIN | ON "),
    template(
        "cte",
        "WITH … AS (…) SELECT",
        "WITH | AS (\n    SELECT \n)\nSELECT * FROM ",
    ),
    template(
        "ct",
        "CREATE TABLE",
        "CREATE TABLE | (\n    id bigint PRIMARY KEY\n)",
    ),
    template("grp", "GROUP BY … HAVING", "GROUP BY | HAVING "),
    template(
        "case",
        "CASE WHEN … THEN … ELSE … END",
        "CASE WHEN | THEN  ELSE  END",
    ),
    template("tx", "BEGIN … COMMIT", "BEGIN;\n|\nCOMMIT;"),
    template(
        "exists",
        "WHERE EXISTS (SELECT …)",
        "WHERE EXISTS (SELECT 1 FROM | WHERE )",
    ),
];

/// The templates whose abbreviation starts with `prefix`, ignoring case.
pub fn templates_for(prefix: &str) -> impl Iterator<Item = &'static Template> + '_ {
    TEMPLATES.iter().filter(move |template| {
        !prefix.is_empty()
            && template
                .abbreviation
                .get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviations_are_unique_and_bodies_have_a_caret() {
        let mut seen = std::collections::HashSet::new();
        for template in TEMPLATES {
            assert!(
                seen.insert(template.abbreviation()),
                "{}",
                template.abbreviation()
            );
            assert_eq!(template.body().matches(CARET).count(), 1);
        }
    }

    #[test]
    fn a_prefix_finds_its_templates() {
        let found: Vec<_> = templates_for("SEL").map(|t| t.abbreviation()).collect();
        assert_eq!(found, ["sel", "selw", "selc", "seld"]);
        assert_eq!(templates_for("").count(), 0);
    }
}
