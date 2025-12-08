//! ngwa-core: Domain models for the ngwa workflow automation system
//!
//! This crate defines the core types used throughout ngwa:
//! - Workflow: A DAG of connected nodes
//! - WorkflowNode: A single node in a workflow
//! - WorkflowEdge: A connection between nodes
//! - NodeDefinition: Trait for implementing node types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

/// A workflow is a directed acyclic graph (DAG) of nodes connected by edges
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub id: Uuid,
    pub name: String,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub settings: WorkflowSettings,
}

impl Workflow {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            nodes: Vec::new(),
            edges: Vec::new(),
            settings: WorkflowSettings::default(),
        }
    }

    pub fn add_node(&mut self, node: WorkflowNode) -> Uuid {
        let id = node.id;
        self.nodes.push(node);
        id
    }

    pub fn add_edge(&mut self, edge: WorkflowEdge) {
        self.edges.push(edge);
    }

    pub fn get_node(&self, id: Uuid) -> Option<&WorkflowNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn get_node_mut(&mut self, id: Uuid) -> Option<&mut WorkflowNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
}

/// Settings for a workflow
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowSettings {
    pub timezone: Option<String>,
    pub error_handling: ErrorHandling,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum ErrorHandling {
    #[default]
    StopOnError,
    ContinueOnError,
}

/// A single node in a workflow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: Uuid,
    pub node_type: String,
    pub name: String,
    pub position: (f32, f32),
    pub config: serde_json::Value,
    pub credentials: Option<Uuid>,
    pub disabled: bool,
}

impl WorkflowNode {
    pub fn new(node_type: impl Into<String>, position: (f32, f32)) -> Self {
        let node_type = node_type.into();
        Self {
            id: Uuid::new_v4(),
            name: node_type.clone(),
            node_type,
            position,
            config: serde_json::Value::Object(Default::default()),
            credentials: None,
            disabled: false,
        }
    }

    pub fn with_id(mut self, id: Uuid) -> Self {
        self.id = id;
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = config;
        self
    }
}

/// An edge connecting two nodes via their pins
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub from_node: Uuid,
    pub from_output: String,
    pub to_node: Uuid,
    pub to_input: String,
}

impl WorkflowEdge {
    pub fn new(
        from_node: Uuid,
        from_output: impl Into<String>,
        to_node: Uuid,
        to_input: impl Into<String>,
    ) -> Self {
        Self {
            from_node,
            from_output: from_output.into(),
            to_node,
            to_input: to_input.into(),
        }
    }
}

/// Definition of a pin (input or output) on a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinDefinition {
    pub name: String,
    pub pin_type: PinType,
    pub required: bool,
    pub description: Option<String>,
}

impl PinDefinition {
    pub fn new(name: impl Into<String>, pin_type: PinType) -> Self {
        Self {
            name: name.into(),
            pin_type,
            required: true,
            description: None,
        }
    }

    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Types of data that can flow through pins
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PinType {
    /// Trigger signal (no data, just execution flow)
    Trigger,
    /// String data
    String,
    /// Numeric data
    Number,
    /// Boolean data
    Boolean,
    /// JSON object/array
    Json,
    /// Binary data
    Binary,
    /// Any type (accepts anything)
    Any,
}

/// Output from a node execution
#[derive(Debug, Clone, Default)]
pub struct NodeOutput {
    pub data: HashMap<String, serde_json::Value>,
}

impl NodeOutput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Serialize) -> Self {
        self.data.insert(
            key.into(),
            serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
        );
        self
    }

    pub fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        self.data
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

/// Context provided to nodes during execution
#[derive(Debug)]
pub struct ExecutionContext {
    pub workflow_id: Uuid,
    pub node_id: Uuid,
    pub inputs: HashMap<String, serde_json::Value>,
    pub config: serde_json::Value,
}

impl ExecutionContext {
    pub fn get_input<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<T, ExecutionError> {
        let value = self
            .inputs
            .get(name)
            .ok_or_else(|| ExecutionError::MissingInput(name.to_string()))?;
        serde_json::from_value(value.clone())
            .map_err(|e| ExecutionError::InvalidInput(name.to_string(), e.to_string()))
    }

    pub fn get_input_optional<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Option<T> {
        self.inputs
            .get(name)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    pub fn get_config<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<T, ExecutionError> {
        let value = self
            .config
            .get(key)
            .ok_or_else(|| ExecutionError::MissingConfig(key.to_string()))?;
        serde_json::from_value(value.clone())
            .map_err(|e| ExecutionError::InvalidConfig(key.to_string(), e.to_string()))
    }
}

/// Errors that can occur during node execution
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Missing required input: {0}")]
    MissingInput(String),

    #[error("Invalid input '{0}': {1}")]
    InvalidInput(String, String),

    #[error("Missing required config: {0}")]
    MissingConfig(String),

    #[error("Invalid config '{0}': {1}")]
    InvalidConfig(String, String),

    #[error("Execution failed: {0}")]
    Failed(String),

    #[error("Node not found: {0}")]
    NodeNotFound(Uuid),

    #[error("Cycle detected in workflow")]
    CycleDetected,
}

/// Boxed future for async node execution
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Trait that all node types must implement
pub trait NodeDefinition: Send + Sync {
    /// Unique identifier for this node type (e.g., "http_request")
    fn node_type(&self) -> &str;

    /// Human-readable name (e.g., "HTTP Request")
    fn display_name(&self) -> &str;

    /// Description of what this node does
    fn description(&self) -> &str {
        ""
    }

    /// Category for grouping in the node palette
    fn category(&self) -> &str {
        "General"
    }

    /// Input pins this node accepts
    fn inputs(&self) -> Vec<PinDefinition>;

    /// Output pins this node produces
    fn outputs(&self) -> Vec<PinDefinition>;

    /// JSON Schema for node configuration
    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }

    /// Execute the node with the given context
    fn execute<'a>(
        &'a self,
        ctx: &'a ExecutionContext,
    ) -> BoxFuture<'a, Result<NodeOutput, ExecutionError>>;
}

/// Data passed to trigger a workflow execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TriggerData {
    pub trigger_type: TriggerType,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum TriggerType {
    #[default]
    Manual,
    Cron,
    Webhook,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_creation() {
        let mut workflow = Workflow::new("Test Workflow");
        assert_eq!(workflow.name, "Test Workflow");

        let node = WorkflowNode::new("http_request", (100.0, 100.0));
        let node_id = workflow.add_node(node);

        assert!(workflow.get_node(node_id).is_some());
    }

    #[test]
    fn test_node_output() {
        let output = NodeOutput::new()
            .with("status", 200)
            .with("body", serde_json::json!({"ok": true}));

        assert_eq!(output.get::<i32>("status"), Some(200));
    }
}
