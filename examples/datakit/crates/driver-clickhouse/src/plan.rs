//! ClickHouse's `EXPLAIN json = 1` as a [`PlanNode`] tree.
//!
//! ClickHouse plans have no costs and, without running the statement, no
//! row counts; what they do say is which steps read which table and how much
//! of it each index ruled out, which become properties of the reading step.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use datakit_driver::PlanNode;
use serde_json::Value as Json;

/// Keys the plan tree shows in places of their own, or not at all.
const SHOWN: &[&str] = &["Node Type", "Node Id", "Description", "Plans", "Indexes"];

/// The plan in `text`, the single value `EXPLAIN json = 1` returns.
pub(crate) fn parse(text: &str) -> Result<PlanNode> {
    let json: Json = serde_json::from_str(text).context("The plan is not valid JSON")?;
    let top = json
        .as_array()
        .and_then(|plans| plans.first())
        .context("The plan is empty")?;
    let root = top.get("Plan").context("The plan has no root")?;
    Ok(parse_node(root))
}

fn parse_node(json: &Json) -> PlanNode {
    let text = |key: &str| json.get(key).and_then(Json::as_str);
    let operation = text("Node Type").unwrap_or("?");
    let mut node = PlanNode::new(operation);
    // A reading step's description is the table it reads; any other step's
    // says what the step is for, such as `(Projection + Before ORDER BY)`.
    if let Some(description) = text("Description").filter(|text| !text.is_empty()) {
        node = if operation.starts_with("ReadFrom") {
            node.with_target(description)
        } else {
            node.with_property("Description", description)
        };
    }
    for index in json
        .get("Indexes")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let (name, summary) = index_summary(index);
        node = node.with_property(name, summary);
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

/// One index a reading step used, as a property: `Index: PrimaryKey` with
/// its keys, condition and how many parts and granules it kept.
fn index_summary(index: &Json) -> (String, String) {
    let text = |key: &str| index.get(key).map(scalar);
    let name = match (text("Type"), text("Name")) {
        (Some(kind), Some(name)) => format!("Index: {kind} {name}"),
        (Some(kind), None) => format!("Index: {kind}"),
        (None, _) => "Index".to_string(),
    };
    let mut parts = Vec::new();
    for key in ["Keys", "Description", "Condition"] {
        if let Some(value) = text(key).filter(|value| !value.is_empty()) {
            parts.push(format!("{key}: {value}"));
        }
    }
    for unit in ["Parts", "Granules"] {
        if let (Some(selected), Some(initial)) = (
            text(&format!("Selected {unit}")),
            text(&format!("Initial {unit}")),
        ) {
            parts.push(format!("{unit}: {selected}/{initial}"));
        }
    }
    (name, parts.join("; "))
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

    /// What ClickHouse 24 prints for
    /// `EXPLAIN json = 1, description = 1, indexes = 1 SELECT … WHERE x > 10`.
    const SAMPLE: &str = r#"[
      {
        "Plan": {
          "Node Type": "Expression",
          "Node Id": "Expression_5",
          "Description": "(Projection + Before ORDER BY)",
          "Plans": [
            {
              "Node Type": "Filter",
              "Node Id": "Filter_3",
              "Description": "WHERE",
              "Plans": [
                {
                  "Node Type": "ReadFromMergeTree",
                  "Node Id": "ReadFromMergeTree_0",
                  "Description": "default.test_table",
                  "Indexes": [
                    {
                      "Type": "PrimaryKey",
                      "Keys": ["x", "y"],
                      "Condition": "(x in [11, +Inf))",
                      "Initial Parts": 3,
                      "Selected Parts": 2,
                      "Initial Granules": 10,
                      "Selected Granules": 6
                    },
                    {
                      "Type": "Skip",
                      "Name": "t_minmax",
                      "Description": "minmax GRANULARITY 2",
                      "Initial Parts": 2,
                      "Selected Parts": 1,
                      "Initial Granules": 6,
                      "Selected Granules": 2
                    }
                  ]
                }
              ]
            }
          ]
        }
      }
    ]"#;

    #[test]
    fn a_json_plan_becomes_a_tree() {
        let plan = parse(SAMPLE).unwrap();
        assert_eq!(plan.operation(), "Expression");
        assert_eq!(
            plan.properties(),
            [(
                "Description".into(),
                "(Projection + Before ORDER BY)".into()
            )]
        );
        let read = &plan.children()[0].children()[0];
        assert_eq!(read.operation(), "ReadFromMergeTree");
        assert_eq!(read.target(), Some("default.test_table"));
        assert_eq!(
            read.properties(),
            [
                (
                    Arc::from("Index: PrimaryKey"),
                    Arc::from(
                        "Keys: x, y; Condition: (x in [11, +Inf)); Parts: 2/3; Granules: 6/10"
                    )
                ),
                (
                    Arc::from("Index: Skip t_minmax"),
                    Arc::from("Description: minmax GRANULARITY 2; Parts: 1/2; Granules: 2/6")
                ),
            ]
        );
        let depths: Vec<usize> = plan.flatten().iter().map(|(depth, _)| *depth).collect();
        assert_eq!(depths, [0, 1, 2]);
    }

    #[test]
    fn text_that_is_not_a_plan_is_an_error() {
        assert!(parse("Expression ((Projection))").is_err());
        assert!(parse("[]").is_err());
    }
}
