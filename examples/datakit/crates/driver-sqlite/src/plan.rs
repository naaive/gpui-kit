//! SQLite's `EXPLAIN QUERY PLAN` as a [`PlanNode`] tree.
//!
//! Each row is one step: `id`, the `parent` step's id (0 for a top-level
//! step), an unused column and the `detail` text, such as
//! `SEARCH orders USING INDEX orders_customer (customer_id=?)`. SQLite gives
//! no costs or row estimates, so the nodes carry only what they do and to
//! what.

use std::collections::HashMap;

use anyhow::{Context as _, Result, bail};
use datakit_driver::{PlanNode, Value};

/// The plan in `rows`, the rows `EXPLAIN QUERY PLAN` returned. A plan with
/// several top-level steps hangs them under one `QUERY PLAN` node.
pub(crate) fn parse(rows: &[Vec<Value>]) -> Result<PlanNode> {
    let mut steps: Vec<(i64, i64, String)> = Vec::with_capacity(rows.len());
    for row in rows {
        let [id, parent, _, detail, ..] = row.as_slice() else {
            bail!("A plan row has {} columns, not 4", row.len());
        };
        let id = integer(id).context("A plan step has no id")?;
        let parent = integer(parent).unwrap_or(0);
        let detail = detail.display().unwrap_or_default().into_owned();
        steps.push((id, parent, detail));
    }
    if steps.is_empty() {
        bail!("The plan is empty");
    }

    let mut children: HashMap<i64, Vec<usize>> = HashMap::new();
    let known: std::collections::HashSet<i64> = steps.iter().map(|(id, _, _)| *id).collect();
    for (ix, (_, parent, _)) in steps.iter().enumerate() {
        // A step whose parent is missing is shown at the top rather than
        // lost.
        let parent = if known.contains(parent) { *parent } else { 0 };
        children.entry(parent).or_default().push(ix);
    }

    fn build(
        ix: usize,
        steps: &[(i64, i64, String)],
        children: &HashMap<i64, Vec<usize>>,
    ) -> PlanNode {
        let (id, _, detail) = &steps[ix];
        let nested = children
            .get(id)
            .map(|nested| {
                nested
                    .iter()
                    .filter(|&&child| child != ix)
                    .map(|&child| build(child, steps, children))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        step(detail).with_children(nested)
    }

    let top: Vec<PlanNode> = children
        .get(&0)
        .map(|top| top.iter().map(|&ix| build(ix, &steps, &children)).collect())
        .unwrap_or_default();
    Ok(match <[PlanNode; 1]>::try_from(top) {
        Ok([root]) => root,
        Err(top) => PlanNode::new("QUERY PLAN").with_children(top),
    })
}

/// One step from its detail text. A scan or search names its table as the
/// target and how it reads it as a property; any other step is its text.
fn step(detail: &str) -> PlanNode {
    for operation in ["SCAN", "SEARCH"] {
        let Some(rest) = detail
            .strip_prefix(operation)
            .and_then(|rest| rest.strip_prefix(' '))
        else {
            continue;
        };
        let (target, how) = match rest.find(" USING ") {
            Some(at) => (&rest[..at], Some(rest[at + 1..].trim())),
            None => (rest, None),
        };
        let mut node = PlanNode::new(operation).with_target(target.trim());
        if let Some(how) = how {
            node = node.with_property("Using", how.trim_start_matches("USING "));
        }
        return node;
    }
    PlanNode::new(detail)
}

fn integer(value: &Value) -> Option<i64> {
    match value {
        Value::Int(value) => Some(*value),
        Value::Text(text) => text.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, parent: i64, detail: &str) -> Vec<Value> {
        vec![
            Value::Int(id),
            Value::Int(parent),
            Value::Int(0),
            Value::Text(detail.into()),
        ]
    }

    #[test]
    fn rows_become_a_tree_by_their_parents() {
        let plan = parse(&[
            row(1, 0, "COMPOUND QUERY"),
            row(2, 1, "LEFT-MOST SUBQUERY"),
            row(3, 2, "SCAN orders"),
            row(6, 1, "UNION ALL"),
            row(7, 6, "SEARCH customers USING INTEGER PRIMARY KEY (rowid=?)"),
        ])
        .unwrap();
        assert_eq!(plan.operation(), "COMPOUND QUERY");
        assert_eq!(plan.children().len(), 2);
        let scan = &plan.children()[0].children()[0];
        assert_eq!((scan.operation(), scan.target()), ("SCAN", Some("orders")));
        let search = &plan.children()[1].children()[0];
        assert_eq!(search.target(), Some("customers"));
        assert_eq!(
            search.properties(),
            [("Using".into(), "INTEGER PRIMARY KEY (rowid=?)".into())]
        );
    }

    #[test]
    fn several_top_level_steps_share_a_root() {
        let plan = parse(&[
            row(4, 0, "SCAN c"),
            row(6, 0, "SEARCH p USING INDEX p_code (code=?)"),
            row(26, 0, "USE TEMP B-TREE FOR ORDER BY"),
        ])
        .unwrap();
        assert_eq!(plan.operation(), "QUERY PLAN");
        let operations: Vec<&str> = plan.children().iter().map(PlanNode::operation).collect();
        assert_eq!(
            operations,
            ["SCAN", "SEARCH", "USE TEMP B-TREE FOR ORDER BY"]
        );
    }

    #[test]
    fn an_empty_plan_is_an_error() {
        assert!(parse(&[]).is_err());
    }
}
