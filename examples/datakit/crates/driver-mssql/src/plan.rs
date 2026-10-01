//! SQL Server's showplan XML as a [`PlanNode`] tree.
//!
//! A showplan document holds one `StmtSimple` per statement, each with a
//! `QueryPlan` whose `RelOp` elements nest the way the operators feed each
//! other. An operator's inputs are the `RelOp`s inside it that no other
//! `RelOp` in between contains: they sit inside an element named after the
//! operator (`<NestedLoops>`, `<Hash>`), not directly inside the `RelOp`.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use datakit_driver::PlanNode;
use roxmltree::{Document, Node};

/// The plan in `documents`, the showplan XML of each statement or batch.
/// One statement is its own root; several hang under a `Batch` node.
pub(crate) fn parse(documents: &[String]) -> Result<PlanNode> {
    let mut statements = Vec::new();
    for text in documents {
        let document = Document::parse(text).context("The plan is not valid XML")?;
        for plan in document
            .descendants()
            .filter(|node| node.has_tag_name("QueryPlan"))
        {
            let Some(root) = plan.children().find(|node| node.has_tag_name("RelOp")) else {
                continue;
            };
            let mut node = parse_operator(root);
            if let Some(statement) = plan.parent_element() {
                for (attribute, name) in [
                    ("StatementText", "Statement"),
                    ("StatementOptmLevel", "Optimization Level"),
                ] {
                    if let Some(value) = statement.attribute(attribute) {
                        node = node.with_property(name, value.trim());
                    }
                }
            }
            for (attribute, name) in [
                ("DegreeOfParallelism", "Degree of Parallelism"),
                ("CompileTime", "Compile Time (ms)"),
            ] {
                if let Some(value) = plan.attribute(attribute) {
                    node = node.with_property(name, value);
                }
            }
            if let Some(times) = plan.children().find(|n| n.has_tag_name("QueryTimeStats")) {
                for (attribute, name) in [
                    ("CpuTime", "CPU Time (ms)"),
                    ("ElapsedTime", "Elapsed Time (ms)"),
                ] {
                    if let Some(value) = times.attribute(attribute) {
                        node = node.with_property(name, value);
                    }
                }
            }
            statements.push(node);
        }
    }
    match statements.len() {
        0 => anyhow::bail!("The plan has no statements"),
        1 => Ok(statements.remove(0)),
        _ => Ok(PlanNode::new("Batch").with_children(statements)),
    }
}

/// Attributes of a `RelOp` shown as properties, with the names Management
/// Studio gives them.
const PROPERTIES: &[(&str, &str)] = &[
    ("EstimateIO", "Estimated I/O Cost"),
    ("EstimateCPU", "Estimated CPU Cost"),
    ("AvgRowSize", "Estimated Row Size"),
    ("EstimatedExecutionMode", "Estimated Execution Mode"),
    ("NodeId", "Node ID"),
];

fn parse_operator(relop: Node) -> PlanNode {
    let number = |name: &str| relop.attribute(name).and_then(|value| value.parse().ok());
    let physical = relop.attribute("PhysicalOp").unwrap_or("?");
    let logical = relop.attribute("LogicalOp");
    let operation = match logical {
        Some(logical) if logical != physical => format!("{physical} ({logical})"),
        _ => physical.to_string(),
    };
    let mut node = PlanNode::new(operation)
        .with_cost(None, number("EstimatedTotalSubtreeCost"))
        .with_estimated_rows(number("EstimateRows"));

    if let Some(runtime) = relop
        .children()
        .find(|child| child.has_tag_name("RunTimeInformation"))
    {
        let threads: Vec<Node> = runtime
            .children()
            .filter(|child| child.has_tag_name("RunTimeCountersPerThread"))
            .collect();
        let sum = |name: &str| -> Option<f64> {
            threads
                .iter()
                .map(|thread| thread.attribute(name)?.parse::<f64>().ok())
                .sum()
        };
        let elapsed = threads
            .iter()
            .filter_map(|thread| thread.attribute("ActualElapsedms")?.parse::<f64>().ok())
            .reduce(f64::max);
        let rows = sum("ActualRows");
        let loops = sum("ActualExecutions");
        // DataKit's figures are per loop, like PostgreSQL's; SQL Server
        // reports totals.
        let per_loop = |total: Option<f64>| match loops {
            Some(loops) if loops > 0.0 => total.map(|total| total / loops),
            _ => total,
        };
        node = node.with_actual(per_loop(elapsed), per_loop(rows), loops);
    }

    if let Some(target) = own(relop, "Object").next().map(target) {
        node = node.with_target(target);
    }
    if let Some(logical) = logical.filter(|logical| *logical != physical) {
        node = node.with_property("Logical Operation", logical);
    }
    for (attribute, name) in PROPERTIES {
        if let Some(value) = relop.attribute(*attribute) {
            node = node.with_property(*name, value);
        }
    }
    if relop
        .attribute("Parallel")
        .is_some_and(|value| value == "1" || value == "true")
    {
        node = node.with_property("Parallel", "true");
    }
    if let Some(predicate) = own(relop, "Predicate")
        .next()
        .and_then(|predicate| {
            predicate
                .descendants()
                .find(|node| node.has_tag_name("ScalarOperator"))
        })
        .and_then(|operator| operator.attribute("ScalarString"))
    {
        node = node.with_property("Predicate", predicate);
    }

    let children: Vec<PlanNode> = own(relop, "RelOp").map(parse_operator).collect();
    node.with_children(children)
}

/// The elements named `name` inside `relop` that belong to it rather than
/// to an operator nested in it.
fn own<'a, 'input>(
    relop: Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    relop.descendants().skip(1).filter(move |node| {
        node.has_tag_name(name)
            && node
                .ancestors()
                .skip(1)
                .find(|ancestor| ancestor.has_tag_name("RelOp"))
                == Some(relop)
    })
}

/// `schema.table using index as alias`, from an `<Object>` element whose
/// names are bracketed.
fn target(object: Node) -> Arc<str> {
    let name = |attribute: &str| {
        object
            .attribute(attribute)
            .map(|value| value.trim_start_matches('[').trim_end_matches(']'))
    };
    let mut target = match (name("Schema"), name("Table")) {
        (Some(schema), Some(table)) => format!("{schema}.{table}"),
        (None, Some(table)) => table.to_string(),
        _ => String::new(),
    };
    if let Some(index) = name("Index") {
        if target.is_empty() {
            target = index.to_string();
        } else {
            target = format!("{target} using {index}");
        }
    }
    if let Some(alias) = name("Alias")
        && !target.ends_with(alias)
    {
        target = format!("{target} as {alias}");
    }
    target.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual plan of a join, trimmed from what SQL Server 2022 returns
    /// for `SET STATISTICS XML ON`.
    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-16"?>
<ShowPlanXML xmlns="http://schemas.microsoft.com/sqlserver/2004/07/showplan" Version="1.564" Build="16.0.1000.6">
  <BatchSequence>
    <Batch>
      <Statements>
        <StmtSimple StatementText="SELECT o.id, c.name FROM dbo.orders o JOIN dbo.customers c ON c.id = o.customer_id WHERE o.total &gt; 100" StatementId="1" StatementCompId="1" StatementType="SELECT" StatementSubTreeCost="0.0099" StatementOptmLevel="FULL">
          <QueryPlan DegreeOfParallelism="1" CompileTime="3" CachedPlanSize="32">
            <QueryTimeStats CpuTime="1" ElapsedTime="2" />
            <RelOp NodeId="0" PhysicalOp="Nested Loops" LogicalOp="Inner Join" EstimateRows="4" EstimateIO="0" EstimateCPU="1.672E-05" AvgRowSize="65" EstimatedTotalSubtreeCost="0.0099" Parallel="0" EstimatedExecutionMode="Row">
              <OutputList />
              <RunTimeInformation>
                <RunTimeCountersPerThread Thread="0" ActualRows="3" ActualEndOfScans="1" ActualExecutions="1" ActualElapsedms="0" />
              </RunTimeInformation>
              <NestedLoops Optimized="0">
                <RelOp NodeId="1" PhysicalOp="Clustered Index Scan" LogicalOp="Clustered Index Scan" EstimateRows="4" EstimatedTotalSubtreeCost="0.0032" Parallel="0">
                  <OutputList />
                  <RunTimeInformation>
                    <RunTimeCountersPerThread Thread="0" ActualRows="3" ActualExecutions="1" ActualElapsedms="0" />
                  </RunTimeInformation>
                  <IndexScan Ordered="0" ForcedIndex="0">
                    <Object Database="[shop]" Schema="[dbo]" Table="[orders]" Index="[PK_orders]" Alias="[o]" IndexKind="Clustered" Storage="RowStore" />
                    <Predicate>
                      <ScalarOperator ScalarString="[shop].[dbo].[orders].[total] as [o].[total]&gt;(100.00)" />
                    </Predicate>
                  </IndexScan>
                </RelOp>
                <RelOp NodeId="2" PhysicalOp="Clustered Index Seek" LogicalOp="Clustered Index Seek" EstimateRows="1" EstimatedTotalSubtreeCost="0.0066" Parallel="0">
                  <OutputList />
                  <RunTimeInformation>
                    <RunTimeCountersPerThread Thread="0" ActualRows="3" ActualExecutions="3" ActualElapsedms="0" />
                  </RunTimeInformation>
                  <IndexScan Ordered="1" ScanDirection="FORWARD">
                    <Object Database="[shop]" Schema="[dbo]" Table="[customers]" Index="[PK_customers]" Alias="[c]" IndexKind="Clustered" Storage="RowStore" />
                  </IndexScan>
                </RelOp>
              </NestedLoops>
            </RelOp>
          </QueryPlan>
        </StmtSimple>
      </Statements>
    </Batch>
  </BatchSequence>
</ShowPlanXML>"#;

    #[test]
    fn a_showplan_becomes_a_tree() {
        let plan = parse(&[SAMPLE.to_string()]).unwrap();
        assert_eq!(plan.operation(), "Nested Loops (Inner Join)");
        assert_eq!(plan.total_cost(), Some(0.0099));
        assert_eq!(plan.estimated_rows(), Some(4.0));
        assert_eq!(plan.actual_rows(), Some(3.0));
        assert!(
            plan.properties()
                .iter()
                .any(|(key, value)| &**key == "Statement" && value.starts_with("SELECT o.id"))
        );
        // The join's own `Object`s belong to its inputs, not to it.
        assert_eq!(plan.target(), None);

        let [scan, seek] = plan.children() else {
            panic!("two inputs");
        };
        assert_eq!(scan.operation(), "Clustered Index Scan");
        assert_eq!(scan.target(), Some("dbo.orders using PK_orders as o"));
        assert!(
            scan.properties()
                .iter()
                .any(|(key, value)| &**key == "Predicate" && value.contains("[total]>(100.00)"))
        );
        assert_eq!(seek.loops(), Some(3.0));
        assert_eq!(seek.actual_rows(), Some(1.0), "rows are per loop");
        assert!(seek.children().is_empty());
        assert!((plan.exclusive_cost().unwrap() - 0.0001).abs() < 1e-9);
    }

    #[test]
    fn several_statements_hang_under_a_batch() {
        let plan = parse(&[SAMPLE.to_string(), SAMPLE.to_string()]).unwrap();
        assert_eq!(plan.operation(), "Batch");
        assert_eq!(plan.children().len(), 2);
        assert!(parse(&["<ShowPlanXML/>".to_string()]).is_err());
        assert!(parse(&["not xml".to_string()]).is_err());
    }
}
