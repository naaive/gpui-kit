use std::sync::Arc;

/// One step of an execution plan, with the steps that feed it.
///
/// Every number is optional: an estimate-only plan has no actual figures,
/// and databases report different things.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanNode {
    operation: Arc<str>,
    /// What the step works on: a relation, an index, a join condition.
    target: Option<Arc<str>>,
    startup_cost: Option<f64>,
    total_cost: Option<f64>,
    estimated_rows: Option<f64>,
    /// Milliseconds until the last row, per loop.
    actual_time: Option<f64>,
    actual_rows: Option<f64>,
    loops: Option<f64>,
    /// Everything else the database said, in its order.
    properties: Vec<(Arc<str>, Arc<str>)>,
    children: Vec<PlanNode>,
}

impl PlanNode {
    pub fn new(operation: impl Into<Arc<str>>) -> Self {
        Self {
            operation: operation.into(),
            ..Default::default()
        }
    }

    pub fn with_target(mut self, target: impl Into<Arc<str>>) -> Self {
        self.target = Some(target.into());
        self
    }

    pub fn with_cost(mut self, startup: Option<f64>, total: Option<f64>) -> Self {
        self.startup_cost = startup;
        self.total_cost = total;
        self
    }

    pub fn with_estimated_rows(mut self, rows: Option<f64>) -> Self {
        self.estimated_rows = rows;
        self
    }

    pub fn with_actual(mut self, time: Option<f64>, rows: Option<f64>, loops: Option<f64>) -> Self {
        self.actual_time = time;
        self.actual_rows = rows;
        self.loops = loops;
        self
    }

    pub fn with_property(mut self, name: impl Into<Arc<str>>, value: impl Into<Arc<str>>) -> Self {
        self.properties.push((name.into(), value.into()));
        self
    }

    pub fn with_children(mut self, children: impl IntoIterator<Item = PlanNode>) -> Self {
        self.children = children.into_iter().collect();
        self
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    pub fn startup_cost(&self) -> Option<f64> {
        self.startup_cost
    }

    pub fn total_cost(&self) -> Option<f64> {
        self.total_cost
    }

    pub fn estimated_rows(&self) -> Option<f64> {
        self.estimated_rows
    }

    pub fn actual_time(&self) -> Option<f64> {
        self.actual_time
    }

    pub fn actual_rows(&self) -> Option<f64> {
        self.actual_rows
    }

    pub fn loops(&self) -> Option<f64> {
        self.loops
    }

    /// Milliseconds the step took over all its loops.
    pub fn total_time(&self) -> Option<f64> {
        Some(self.actual_time? * self.loops.unwrap_or(1.0))
    }

    /// Milliseconds spent in this step, without the steps feeding it.
    pub fn exclusive_time(&self) -> Option<f64> {
        let children: f64 = self.children.iter().filter_map(PlanNode::total_time).sum();
        Some((self.total_time()? - children).max(0.0))
    }

    /// The step's own cost, without the cost of the steps feeding it.
    pub fn exclusive_cost(&self) -> Option<f64> {
        let children: f64 = self.children.iter().filter_map(|c| c.total_cost).sum();
        Some((self.total_cost? - children).max(0.0))
    }

    pub fn properties(&self) -> &[(Arc<str>, Arc<str>)] {
        &self.properties
    }

    pub fn children(&self) -> &[PlanNode] {
        &self.children
    }

    /// Every step, depth first, with its depth.
    pub fn flatten(&self) -> Vec<(usize, &PlanNode)> {
        let mut nodes = Vec::new();
        let mut stack = vec![(0, self)];
        while let Some((depth, node)) = stack.pop() {
            nodes.push((depth, node));
            stack.extend(node.children.iter().rev().map(|child| (depth + 1, child)));
        }
        nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_time_subtracts_the_children() {
        let plan = PlanNode::new("Hash Join")
            .with_actual(Some(10.0), Some(5.0), Some(1.0))
            .with_children([
                PlanNode::new("Seq Scan").with_actual(Some(2.0), Some(5.0), Some(2.0)),
                PlanNode::new("Hash").with_actual(Some(3.0), Some(1.0), Some(1.0)),
            ]);
        assert_eq!(plan.exclusive_time(), Some(3.0));
        let depths: Vec<usize> = plan.flatten().iter().map(|(depth, _)| *depth).collect();
        assert_eq!(depths, vec![0, 1, 1]);
    }
}
