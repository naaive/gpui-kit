//! PostgreSQL's `EXPLAIN (FORMAT JSON)` as a [`PlanNode`] tree.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use datakit_driver::PlanNode;
use serde_json::Value as Json;

/// Keys the plan tree shows in columns of their own.
const SHOWN: &[&str] = &[
    "Node Type",
    "Join Type",
    "Plans",
    "Startup Cost",
    "Total Cost",
    "Plan Rows",
    "Actual Startup Time",
    "Actual Total Time",
    "Actual Rows",
    "Actual Loops",
    "Relation Name",
    "Schema",
    "Alias",
    "Index Name",
    "Parent Relationship",
];

/// The plan in `text`, the single value `EXPLAIN (FORMAT JSON)` returns.
/// Planning and execution time become properties of the root.
pub(crate) fn parse(text: &str) -> Result<PlanNode> {
    let json: Json = serde_json::from_str(text).context("The plan is not valid JSON")?;
    let top = json
        .as_array()
        .and_then(|plans| plans.first())
        .context("The plan is empty")?;
    let root = top.get("Plan").context("The plan has no root")?;
    let mut node = parse_node(root);
    for key in ["Planning Time", "Execution Time"] {
        if let Some(value) = top.get(key) {
            node = node.with_property(key, format!("{} ms", scalar(value)));
        }
    }
    Ok(node)
}

fn parse_node(json: &Json) -> PlanNode {
    let number = |key: &str| json.get(key).and_then(Json::as_f64);
    let text = |key: &str| json.get(key).and_then(Json::as_str);
    let mut operation = text("Node Type").unwrap_or("?").to_string();
    if let Some(join) = text("Join Type")
        && operation.ends_with("Join")
        && join != "Inner"
    {
        operation = format!("{operation} ({join})");
    }
    let target = match (text("Schema"), text("Relation Name"), text("Index Name")) {
        (_, Some(relation), Some(index)) => Some(format!("{relation} using {index}")),
        (Some(schema), Some(relation), None) => Some(format!("{schema}.{relation}")),
        (None, Some(relation), None) => Some(relation.to_string()),
        (_, None, Some(index)) => Some(index.to_string()),
        _ => None,
    };
    let target = match (target, text("Alias")) {
        (Some(target), Some(alias)) if !target.ends_with(alias) => {
            Some(format!("{target} as {alias}"))
        }
        (target, _) => target,
    };
    let mut node = PlanNode::new(operation)
        .with_cost(number("Startup Cost"), number("Total Cost"))
        .with_estimated_rows(number("Plan Rows"))
        .with_actual(
            number("Actual Total Time"),
            number("Actual Rows"),
            number("Actual Loops"),
        );
    if let Some(target) = target {
        node = node.with_target(target);
    }
    if let Some(object) = json.as_object() {
        for (key, value) in object {
            if !SHOWN.contains(&key.as_str()) {
                node = node.with_property(key.as_str(), scalar(value));
            }
        }
    }
    let children = json
        .get("Plans")
        .and_then(Json::as_array)
        .map(|plans| plans.iter().map(parse_node).collect::<Vec<_>>())
        .unwrap_or_default();
    node.with_children(children)
}

fn scalar(value: &Json) -> Arc<str> {
    match value {
        Json::String(text) => text.as_str().into(),
        Json::Array(items) => items
            .iter()
            .map(|item| scalar(item).to_string())
            .collect::<Vec<_>>()
            .join(", ")
            .into(),
        other => other.to_string().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_plan_becomes_a_tree() {
        let text = r#"[{"Plan": {"Node Type": "Hash Join", "Join Type": "Left",
            "Startup Cost": 1.5, "Total Cost": 20.0, "Plan Rows": 10,
            "Hash Cond": "(o.customer_id = c.id)",
            "Plans": [
              {"Node Type": "Seq Scan", "Relation Name": "orders", "Schema": "shop",
               "Alias": "o", "Total Cost": 10.0, "Plan Rows": 100},
              {"Node Type": "Hash", "Total Cost": 5.0,
               "Plans": [{"Node Type": "Index Scan", "Relation Name": "customers",
                          "Index Name": "customers_pkey", "Total Cost": 4.0}]}
            ]}, "Planning Time": 0.2}]"#;
        let plan = parse(text).unwrap();
        assert_eq!(plan.operation(), "Hash Join (Left)");
        assert_eq!(plan.children().len(), 2);
        assert_eq!(plan.children()[0].target(), Some("shop.orders as o"));
        assert_eq!(
            plan.children()[1].children()[0].target(),
            Some("customers using customers_pkey")
        );
        assert!(
            plan.properties()
                .iter()
                .any(|(key, value)| &**key == "Hash Cond" && &**value == "(o.customer_id = c.id)")
        );
        assert!(
            plan.properties()
                .iter()
                .any(|(key, _)| &**key == "Planning Time")
        );
        assert_eq!(plan.exclusive_cost(), Some(5.0));
    }
}
