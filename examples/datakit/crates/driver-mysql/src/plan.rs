//! MySQL's execution plans as a [`PlanNode`] tree.
//!
//! `EXPLAIN FORMAT=JSON` answers with nested objects named after what each
//! step does (`query_block`, `ordering_operation`, `nested_loop`, `table`)
//! and estimated figures only. `EXPLAIN ANALYZE` answers with an indented
//! text tree whose lines carry the estimate and the actual figures. Both are
//! read here; so is the JSON format MySQL 8.3 and later write when
//! `explain_json_format_version = 2`, which nests `inputs`.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use datakit_driver::PlanNode;
use serde_json::Value as Json;

/// The plan in `text`, the single value an `EXPLAIN` returns.
pub(crate) fn parse(text: &str) -> Result<PlanNode> {
    let text = text.trim();
    if text.starts_with('{') {
        let json: Json = serde_json::from_str(text).context("The plan is not valid JSON")?;
        if let Some(block) = json.get("query_block") {
            return Ok(query_block(block));
        }
        if json.get("operation").is_some() {
            return Ok(operation(&json));
        }
        bail!("The plan has no query block")
    }
    if text.starts_with("->") {
        return tree(text);
    }
    bail!("The plan is in a format DataKit doesn’t read")
}

// `FORMAT=JSON`, version 1.

/// Keys that hold the steps feeding a step, rather than describe it.
const STEPS: &[&str] = &[
    "query_block",
    "table",
    "nested_loop",
    "ordering_operation",
    "grouping_operation",
    "duplicates_removal",
    "windowing",
    "union_result",
    "materialized_from_subquery",
    "attached_subqueries",
    "optimized_away_subqueries",
    "order_by_subqueries",
    "group_by_subqueries",
    "having_subqueries",
    "select_list_subqueries",
    "update_value_subqueries",
    "query_specifications",
];

fn query_block(json: &Json) -> PlanNode {
    let cost = json
        .get("cost_info")
        .and_then(|cost| number(cost.get("query_cost")));
    step("Query Block", json)
        .with_cost(None, cost)
        .with_children(steps(json))
}

/// A step that only wraps others, like a sort, with its scalar properties.
fn step(operation: &str, json: &Json) -> PlanNode {
    let mut node = PlanNode::new(operation);
    if let Some(object) = json.as_object() {
        for (key, value) in object {
            if !STEPS.contains(&key.as_str()) && key != "cost_info" {
                node = node.with_property(key.as_str(), scalar(value));
            }
        }
    }
    node
}

/// The steps feeding `json`, in the order the plan lists them.
fn steps(json: &Json) -> Vec<PlanNode> {
    let Some(object) = json.as_object() else {
        return Vec::new();
    };
    let mut children = Vec::new();
    for (key, value) in object {
        match key.as_str() {
            "query_block" => children.push(query_block(value)),
            "table" => children.push(table(value)),
            "nested_loop" => {
                let tables: Vec<PlanNode> = value
                    .as_array()
                    .map(|items| items.iter().flat_map(steps).collect())
                    .unwrap_or_default();
                // Each table's prefix cost covers the tables before it, so
                // the last one's is the whole join's.
                let cost = value.as_array().and_then(|items| {
                    items
                        .iter()
                        .rev()
                        .find_map(|item| number(item.pointer("/table/cost_info/prefix_cost")))
                });
                children.push(
                    PlanNode::new("Nested Loop")
                        .with_cost(None, cost)
                        .with_children(tables),
                );
            }
            "ordering_operation" => {
                let operation = if flag(value, "using_filesort") {
                    "Sort"
                } else {
                    "Ordering"
                };
                children.push(step(operation, value).with_children(steps(value)));
            }
            "grouping_operation" => {
                children.push(step("Group", value).with_children(steps(value)));
            }
            "duplicates_removal" => {
                children.push(step("Distinct", value).with_children(steps(value)));
            }
            "windowing" => children.push(step("Window", value).with_children(steps(value))),
            "materialized_from_subquery" => {
                children.push(step("Materialize", value).with_children(steps(value)));
            }
            "union_result" => {
                let mut node = step("Union", value);
                if let Some(name) = value.get("table_name").and_then(Json::as_str) {
                    node = node.with_target(name);
                }
                let blocks: Vec<PlanNode> = value
                    .get("query_specifications")
                    .and_then(Json::as_array)
                    .map(|items| items.iter().flat_map(steps).collect())
                    .unwrap_or_default();
                children.push(node.with_children(blocks));
            }
            key if key.ends_with("_subqueries") => {
                if let Some(items) = value.as_array() {
                    children.extend(items.iter().flat_map(steps));
                }
            }
            _ => {}
        }
    }
    children
}

/// Keys of a table step shown in columns of their own.
const TABLE_SHOWN: &[&str] = &[
    "table_name",
    "access_type",
    "key",
    "rows_produced_per_join",
    "cost_info",
];

/// One table read, named after how it is read.
fn table(json: &Json) -> PlanNode {
    let text = |key: &str| json.get(key).and_then(Json::as_str);
    let access = text("access_type").unwrap_or("");
    let operation = match access {
        "ALL" => "Full Table Scan",
        "index" => "Full Index Scan",
        "range" => "Index Range Scan",
        "ref" => "Index Lookup",
        "ref_or_null" => "Index Lookup or Null",
        "eq_ref" => "Unique Index Lookup",
        "const" | "system" => "Constant Row",
        "fulltext" => "Full-Text Search",
        "index_merge" => "Index Merge",
        "unique_subquery" | "index_subquery" => "Subquery Index Lookup",
        "" => "Table",
        other => other,
    };
    let target = match (text("table_name"), text("key")) {
        (Some(table), Some(key)) => Some(format!("{table} using {key}")),
        (Some(table), None) => Some(table.to_string()),
        (None, Some(key)) => Some(key.to_string()),
        (None, None) => None,
    };
    let cost_info = json.get("cost_info");
    let cost = match (
        number(cost_info.and_then(|cost| cost.get("read_cost"))),
        number(cost_info.and_then(|cost| cost.get("eval_cost"))),
    ) {
        (Some(read), Some(eval)) => Some(read + eval),
        (read, eval) => read.or(eval),
    };
    let mut node = PlanNode::new(operation)
        .with_cost(None, cost)
        .with_estimated_rows(number(json.get("rows_produced_per_join")));
    if let Some(target) = target {
        node = node.with_target(target);
    }
    if let Some(object) = json.as_object() {
        for (key, value) in object {
            if !TABLE_SHOWN.contains(&key.as_str()) && !STEPS.contains(&key.as_str()) {
                node = node.with_property(key.as_str(), scalar(value));
            }
        }
    }
    if let Some(Json::Object(cost)) = cost_info {
        for (key, value) in cost {
            node = node.with_property(key.as_str(), scalar(value));
        }
    }
    node.with_children(steps(json))
}

// `FORMAT=JSON`, version 2.

/// Keys of a version 2 step shown in columns of their own.
const OPERATION_SHOWN: &[&str] = &[
    "operation",
    "inputs",
    "estimated_rows",
    "estimated_total_cost",
    "estimated_first_row_cost",
    "actual_last_row_ms",
    "actual_rows",
    "actual_loops",
    "table_name",
    "index_name",
];

fn operation(json: &Json) -> PlanNode {
    let text = |key: &str| json.get(key).and_then(Json::as_str);
    let mut node = PlanNode::new(text("operation").unwrap_or("?"))
        .with_cost(
            number(json.get("estimated_first_row_cost")),
            number(json.get("estimated_total_cost")),
        )
        .with_estimated_rows(number(json.get("estimated_rows")))
        .with_actual(
            number(json.get("actual_last_row_ms")),
            number(json.get("actual_rows")),
            number(json.get("actual_loops")),
        );
    match (text("table_name"), text("index_name")) {
        (Some(table), Some(index)) => node = node.with_target(format!("{table} using {index}")),
        (Some(table), None) => node = node.with_target(table),
        _ => {}
    }
    if let Some(object) = json.as_object() {
        for (key, value) in object {
            if !OPERATION_SHOWN.contains(&key.as_str()) {
                node = node.with_property(key.as_str(), scalar(value));
            }
        }
    }
    let children = json
        .get("inputs")
        .and_then(Json::as_array)
        .map(|inputs| inputs.iter().map(operation).collect::<Vec<_>>())
        .unwrap_or_default();
    node.with_children(children)
}

// `EXPLAIN ANALYZE`.

/// The indented tree `EXPLAIN ANALYZE` prints, one step per line:
///
/// ```text
/// -> Nested loop inner join  (cost=0.9 rows=1) (actual time=0.02..0.03 rows=3 loops=1)
///     -> Table scan on o  (cost=0.55 rows=3) (actual time=0.01..0.01 rows=3 loops=1)
/// ```
fn tree(text: &str) -> Result<PlanNode> {
    let mut lines: Vec<(usize, String)> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some(step) = trimmed.strip_prefix("->") else {
            // A long condition can run onto the next line.
            if let Some((_, last)) = lines.last_mut() {
                last.push(' ');
                last.push_str(trimmed);
            }
            continue;
        };
        let depth = line.len() - trimmed.len();
        lines.push((depth, step.trim().to_string()));
    }
    let mut steps = lines.iter().map(|(depth, text)| (*depth, line_step(text)));
    let Some((depth, root)) = steps.next() else {
        bail!("The plan is empty")
    };
    let rest: Vec<(usize, PlanNode)> = steps.collect();
    let mut ix = 0;
    Ok(root.with_children(children_at(&rest, &mut ix, depth)))
}

/// The steps from `ix` on that are deeper than `parent`, as a tree.
fn children_at(steps: &[(usize, PlanNode)], ix: &mut usize, parent: usize) -> Vec<PlanNode> {
    let mut children = Vec::new();
    while let Some((depth, node)) = steps.get(*ix) {
        if *depth <= parent {
            break;
        }
        *ix += 1;
        let grandchildren = children_at(steps, ix, *depth);
        children.push(node.clone().with_children(grandchildren));
    }
    children
}

/// One line of the tree: what the step does, then its figures in trailing
/// parentheses.
fn line_step(text: &str) -> PlanNode {
    let mut description = text.trim_end();
    let mut estimate = None;
    let mut actual = None;
    let mut never_executed = false;
    while description.ends_with(')') {
        let Some(open) = matching_open(description) else {
            break;
        };
        let group = &description[open + 1..description.len() - 1];
        if let Some(figures) = group.strip_prefix("cost=") {
            estimate = Some(figures.to_string());
        } else if let Some(figures) = group.strip_prefix("actual time=") {
            actual = Some(figures.to_string());
        } else if group == "never executed" {
            never_executed = true;
        } else {
            break;
        }
        description = description[..open].trim_end();
    }

    let (operation, target) = match description.split_once(": ") {
        Some((operation, target)) => (operation, Some(target)),
        None => match description.split_once(" on ") {
            Some((operation, target)) => (operation, Some(target)),
            None => (description, None),
        },
    };
    let mut node = PlanNode::new(operation);
    if let Some(target) = target {
        node = node.with_target(target);
    }
    if let Some(estimate) = estimate {
        // `0.9 rows=1`, or `0.25..0.9 rows=1` with the first row's cost.
        let (cost, rows) = estimate.split_once(" rows=").unwrap_or((&estimate, ""));
        let (startup, total) = match cost.split_once("..") {
            Some((startup, total)) => (startup.parse().ok(), total.parse().ok()),
            None => (None, cost.parse().ok()),
        };
        node = node
            .with_cost(startup, total)
            .with_estimated_rows(rows.parse().ok());
    }
    if let Some(actual) = actual {
        // `0.0219..0.0279 rows=3 loops=1`: first row..last row, per loop.
        let mut time = None;
        let mut rows = None;
        let mut loops = None;
        for (ix, part) in actual.split(' ').enumerate() {
            if ix == 0 {
                time = part
                    .split_once("..")
                    .and_then(|(_, last)| last.parse().ok());
            } else if let Some(value) = part.strip_prefix("rows=") {
                rows = value.parse().ok();
            } else if let Some(value) = part.strip_prefix("loops=") {
                loops = value.parse().ok();
            }
        }
        node = node.with_actual(time, rows, loops);
    }
    if never_executed {
        node = node.with_property("Never Executed", "true");
    }
    node
}

/// Where the parenthesis closing `text` opens.
fn matching_open(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (ix, c) in text.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    return Some(ix);
                }
            }
            _ => {}
        }
    }
    None
}

/// A figure MySQL writes as a number or, in version 1, as a string.
fn number(value: Option<&Json>) -> Option<f64> {
    match value? {
        Json::Number(number) => number.as_f64(),
        Json::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn flag(json: &Json, key: &str) -> bool {
    json.get(key).and_then(Json::as_bool).unwrap_or(false)
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

    /// What MySQL 8.4 answers for a join that is grouped and sorted.
    const JSON_PLAN: &str = r#"{
      "query_block": {
        "select_id": 1,
        "cost_info": {"query_cost": "0.90"},
        "ordering_operation": {
          "using_filesort": true,
          "grouping_operation": {
            "using_temporary_table": true,
            "using_filesort": false,
            "nested_loop": [
              {"table": {
                "table_name": "o", "access_type": "ALL",
                "possible_keys": ["customer_id"],
                "rows_examined_per_scan": 3, "rows_produced_per_join": 1,
                "filtered": "33.33",
                "cost_info": {"read_cost": "0.45", "eval_cost": "0.10",
                              "prefix_cost": "0.55", "data_read_per_join": "16"},
                "used_columns": ["id", "customer_id", "total"],
                "attached_condition": "((`shop`.`o`.`total` > 1.00) and (`shop`.`o`.`customer_id` is not null))"
              }},
              {"table": {
                "table_name": "c", "access_type": "eq_ref",
                "possible_keys": ["PRIMARY"], "key": "PRIMARY",
                "used_key_parts": ["id"], "key_length": "4",
                "ref": ["shop.o.customer_id"],
                "rows_examined_per_scan": 1, "rows_produced_per_join": 1,
                "filtered": "100.00",
                "cost_info": {"read_cost": "0.25", "eval_cost": "0.10",
                              "prefix_cost": "0.90", "data_read_per_join": "88"},
                "used_columns": ["id", "name"]
              }}
            ]
          }
        }
      }
    }"#;

    #[test]
    fn a_json_plan_becomes_a_tree() {
        let plan = parse(JSON_PLAN).unwrap();
        assert_eq!(plan.operation(), "Query Block");
        assert_eq!(plan.total_cost(), Some(0.9));
        let sort = &plan.children()[0];
        assert_eq!(sort.operation(), "Sort");
        let group = &sort.children()[0];
        assert_eq!(group.operation(), "Group");
        assert!(
            group
                .properties()
                .iter()
                .any(|(key, value)| &**key == "using_temporary_table" && &**value == "true")
        );
        let join = &group.children()[0];
        assert_eq!(join.operation(), "Nested Loop");
        assert_eq!(join.total_cost(), Some(0.9));
        let [orders, customers] = join.children() else {
            panic!("two tables");
        };
        assert_eq!(orders.operation(), "Full Table Scan");
        assert_eq!(orders.target(), Some("o"));
        assert_eq!(orders.estimated_rows(), Some(1.0));
        assert!(
            orders
                .properties()
                .iter()
                .any(|(key, value)| &**key == "attached_condition"
                    && value.contains("`total` > 1.00"))
        );
        assert_eq!(customers.operation(), "Unique Index Lookup");
        assert_eq!(customers.target(), Some("c using PRIMARY"));
        assert!((customers.total_cost().unwrap() - 0.35).abs() < 1e-9);
    }

    #[test]
    fn an_analyzed_tree_carries_actual_figures() {
        let text = "-> Sort: `sum(o.total)` DESC  (actual time=0.121..0.121 rows=2 loops=1)
    -> Table scan on <temporary>  (actual time=0.0835..0.0841 rows=2 loops=1)
        -> Aggregate using temporary table  (actual time=0.0821..0.0821 rows=2 loops=1)
            -> Nested loop inner join  (cost=0.9 rows=1) (actual time=0.0219..0.0279 rows=3 loops=1)
                -> Filter: ((o.total > 1.00) and (o.customer_id is not null))  (cost=0.55 rows=1) (actual time=0.0147..0.0177 rows=3 loops=1)
                    -> Table scan on o  (cost=0.55 rows=3) (actual time=0.0118..0.0142 rows=3 loops=1)
                -> Single-row index lookup on c using PRIMARY (id=o.customer_id)  (cost=0.35 rows=1) (actual time=0.0025..0.00253 rows=1 loops=3)";
        let plan = parse(text).unwrap();
        assert_eq!(plan.operation(), "Sort");
        assert_eq!(plan.target(), Some("`sum(o.total)` DESC"));
        assert_eq!(plan.actual_time(), Some(0.121));
        let join = &plan.children()[0].children()[0].children()[0];
        assert_eq!(join.operation(), "Nested loop inner join");
        assert_eq!(join.total_cost(), Some(0.9));
        assert_eq!(join.actual_rows(), Some(3.0));
        let [filter, lookup] = join.children() else {
            panic!("two inputs");
        };
        assert_eq!(filter.operation(), "Filter");
        assert_eq!(
            filter.target(),
            Some("((o.total > 1.00) and (o.customer_id is not null))")
        );
        assert_eq!(filter.children()[0].target(), Some("o"));
        assert_eq!(lookup.operation(), "Single-row index lookup");
        assert_eq!(lookup.target(), Some("c using PRIMARY (id=o.customer_id)"));
        assert_eq!(lookup.loops(), Some(3.0));
        assert_eq!(lookup.estimated_rows(), Some(1.0));
    }

    #[test]
    fn a_version_two_plan_nests_inputs() {
        let text = r#"{"query": "select 1", "operation": "Nested loop inner join",
            "estimated_total_cost": 1.5, "estimated_rows": 2,
            "inputs": [{"operation": "Table scan on t", "table_name": "t",
                        "access_type": "table", "estimated_total_cost": 0.5}]}"#;
        let plan = parse(text).unwrap();
        assert_eq!(plan.operation(), "Nested loop inner join");
        assert_eq!(plan.children()[0].target(), Some("t"));
        assert_eq!(plan.exclusive_cost(), Some(1.0));
    }
}
