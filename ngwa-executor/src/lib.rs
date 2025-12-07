//! ngwa-executor: Workflow execution engine
//!
//! This crate handles executing workflows by:
//! - Topologically sorting nodes
//! - Executing nodes in order
//! - Passing data between nodes via edges

use ngwa_core::{
    BoxFuture, ExecutionContext, ExecutionError, NodeDefinition, NodeOutput, TriggerData,
    Workflow, WorkflowEdge,
};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Registry of available node types
pub struct NodeRegistry {
    nodes: HashMap<String, Arc<dyn NodeDefinition>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    pub fn register<N: NodeDefinition + 'static>(&mut self, node: N) {
        self.nodes.insert(node.node_type().to_string(), Arc::new(node));
    }

    pub fn get(&self, node_type: &str) -> Option<Arc<dyn NodeDefinition>> {
        self.nodes.get(node_type).cloned()
    }

    pub fn all(&self) -> impl Iterator<Item = &Arc<dyn NodeDefinition>> {
        self.nodes.values()
    }
}

impl Default for NodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a workflow execution
#[derive(Debug)]
pub struct ExecutionResult {
    pub workflow_id: Uuid,
    pub node_outputs: HashMap<Uuid, NodeOutput>,
    pub success: bool,
    pub error: Option<String>,
}

/// Executes workflows
pub struct WorkflowExecutor {
    node_registry: Arc<NodeRegistry>,
}

impl WorkflowExecutor {
    pub fn new(node_registry: Arc<NodeRegistry>) -> Self {
        Self { node_registry }
    }

    /// Execute a workflow with the given trigger data
    pub async fn execute(
        &self,
        workflow: &Workflow,
        trigger_data: TriggerData,
    ) -> Result<ExecutionResult, ExecutionError> {
        // 1. Topological sort
        let execution_order = self.topological_sort(workflow)?;

        // 2. Execute nodes in order
        let mut node_outputs: HashMap<Uuid, NodeOutput> = HashMap::new();

        for node_id in execution_order {
            let node = workflow
                .get_node(node_id)
                .ok_or(ExecutionError::NodeNotFound(node_id))?;

            if node.disabled {
                continue;
            }

            let definition = self
                .node_registry
                .get(&node.node_type)
                .ok_or_else(|| ExecutionError::Failed(format!("Unknown node type: {}", node.node_type)))?;

            // Gather inputs from connected nodes
            let inputs = self.gather_inputs(node_id, &workflow.edges, &node_outputs);

            let ctx = ExecutionContext {
                workflow_id: workflow.id,
                node_id,
                inputs,
                config: node.config.clone(),
            };

            // Execute the node
            let output = definition.execute(&ctx).await?;
            node_outputs.insert(node_id, output);
        }

        Ok(ExecutionResult {
            workflow_id: workflow.id,
            node_outputs,
            success: true,
            error: None,
        })
    }

    /// Perform topological sort on workflow nodes
    fn topological_sort(&self, workflow: &Workflow) -> Result<Vec<Uuid>, ExecutionError> {
        let mut in_degree: HashMap<Uuid, usize> = HashMap::new();
        let mut adjacency: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

        // Initialize
        for node in &workflow.nodes {
            in_degree.insert(node.id, 0);
            adjacency.insert(node.id, Vec::new());
        }

        // Build graph
        for edge in &workflow.edges {
            if let Some(adj) = adjacency.get_mut(&edge.from_node) {
                adj.push(edge.to_node);
            }
            if let Some(deg) = in_degree.get_mut(&edge.to_node) {
                *deg += 1;
            }
        }

        // Kahn's algorithm
        let mut queue: Vec<Uuid> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut result = Vec::new();

        while let Some(node_id) = queue.pop() {
            result.push(node_id);

            if let Some(neighbors) = adjacency.get(&node_id) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(&neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(neighbor);
                        }
                    }
                }
            }
        }

        if result.len() != workflow.nodes.len() {
            return Err(ExecutionError::CycleDetected);
        }

        Ok(result)
    }

    /// Gather input values for a node from connected upstream nodes
    fn gather_inputs(
        &self,
        node_id: Uuid,
        edges: &[WorkflowEdge],
        outputs: &HashMap<Uuid, NodeOutput>,
    ) -> HashMap<String, serde_json::Value> {
        let mut inputs = HashMap::new();

        for edge in edges {
            if edge.to_node == node_id {
                if let Some(output) = outputs.get(&edge.from_node) {
                    if let Some(value) = output.data.get(&edge.from_output) {
                        inputs.insert(edge.to_input.clone(), value.clone());
                    }
                }
            }
        }

        inputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ngwa_core::WorkflowNode;

    #[test]
    fn test_topological_sort() {
        let mut workflow = Workflow::new("Test");

        let node1 = WorkflowNode::new("trigger", (0.0, 0.0));
        let node2 = WorkflowNode::new("process", (100.0, 0.0));
        let node3 = WorkflowNode::new("output", (200.0, 0.0));

        let id1 = node1.id;
        let id2 = node2.id;
        let id3 = node3.id;

        workflow.add_node(node1);
        workflow.add_node(node2);
        workflow.add_node(node3);

        workflow.add_edge(WorkflowEdge::new(id1, "out", id2, "in"));
        workflow.add_edge(WorkflowEdge::new(id2, "out", id3, "in"));

        let registry = NodeRegistry::new();
        let executor = WorkflowExecutor::new(Arc::new(registry));

        let order = executor.topological_sort(&workflow).unwrap();

        // id1 must come before id2, id2 must come before id3
        let pos1 = order.iter().position(|&id| id == id1).unwrap();
        let pos2 = order.iter().position(|&id| id == id2).unwrap();
        let pos3 = order.iter().position(|&id| id == id3).unwrap();

        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
    }
}
