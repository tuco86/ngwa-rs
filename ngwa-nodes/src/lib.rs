//! ngwa-nodes: Built-in node implementations
//!
//! This crate provides the core set of nodes for ngwa:
//! - Triggers: Manual, Cron, Webhook
//! - HTTP: HTTP Request
//! - Logic: If, Switch
//! - Data: Set, Merge

use ngwa_core::{
    BoxFuture, ExecutionContext, ExecutionError, NodeDefinition, NodeOutput, PinDefinition,
    PinType,
};

/// Manual trigger node - starts a workflow manually
pub struct ManualTriggerNode;

impl NodeDefinition for ManualTriggerNode {
    fn node_type(&self) -> &str {
        "manual_trigger"
    }

    fn display_name(&self) -> &str {
        "Manual Trigger"
    }

    fn description(&self) -> &str {
        "Starts the workflow when manually triggered"
    }

    fn category(&self) -> &str {
        "Triggers"
    }

    fn inputs(&self) -> Vec<PinDefinition> {
        vec![]
    }

    fn outputs(&self) -> Vec<PinDefinition> {
        vec![PinDefinition::new("trigger", PinType::Trigger)]
    }

    fn execute<'a>(
        &'a self,
        _ctx: &'a ExecutionContext,
    ) -> BoxFuture<'a, Result<NodeOutput, ExecutionError>> {
        Box::pin(async move {
            Ok(NodeOutput::new().with("trigger", true))
        })
    }
}

/// HTTP Request node - makes HTTP requests
pub struct HttpRequestNode;

impl NodeDefinition for HttpRequestNode {
    fn node_type(&self) -> &str {
        "http_request"
    }

    fn display_name(&self) -> &str {
        "HTTP Request"
    }

    fn description(&self) -> &str {
        "Makes an HTTP request to a URL"
    }

    fn category(&self) -> &str {
        "HTTP"
    }

    fn inputs(&self) -> Vec<PinDefinition> {
        vec![
            PinDefinition::new("trigger", PinType::Trigger),
            PinDefinition::new("url", PinType::String).optional(),
            PinDefinition::new("body", PinType::Json).optional(),
            PinDefinition::new("headers", PinType::Json).optional(),
        ]
    }

    fn outputs(&self) -> Vec<PinDefinition> {
        vec![
            PinDefinition::new("response", PinType::Json),
            PinDefinition::new("status", PinType::Number),
            PinDefinition::new("headers", PinType::Json),
        ]
    }

    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "Request URL" },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "DELETE", "PATCH"],
                    "default": "GET"
                },
                "headers": { "type": "object" },
                "body": { "type": "object" }
            },
            "required": ["url"]
        })
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a ExecutionContext,
    ) -> BoxFuture<'a, Result<NodeOutput, ExecutionError>> {
        Box::pin(async move {
            // Get URL from input or config
            let url: String = ctx
                .get_input_optional("url")
                .or_else(|| ctx.get_config("url").ok())
                .ok_or_else(|| ExecutionError::MissingConfig("url".to_string()))?;

            let method: String = ctx
                .get_config("method")
                .unwrap_or_else(|_| "GET".to_string());

            let client = reqwest::Client::new();

            let mut request = match method.to_uppercase().as_str() {
                "GET" => client.get(&url),
                "POST" => client.post(&url),
                "PUT" => client.put(&url),
                "DELETE" => client.delete(&url),
                "PATCH" => client.patch(&url),
                _ => client.get(&url),
            };

            // Add body if present
            if let Some(body) = ctx.get_input_optional::<serde_json::Value>("body") {
                request = request.json(&body);
            } else if let Ok(body) = ctx.get_config::<serde_json::Value>("body") {
                request = request.json(&body);
            }

            // Add headers if present
            if let Some(headers) = ctx.get_input_optional::<serde_json::Map<String, serde_json::Value>>("headers") {
                for (key, value) in headers {
                    if let Some(v) = value.as_str() {
                        request = request.header(&key, v);
                    }
                }
            }

            let response = request
                .send()
                .await
                .map_err(|e| ExecutionError::Failed(e.to_string()))?;

            let status = response.status().as_u16();
            let headers: serde_json::Map<String, serde_json::Value> = response
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.to_string(),
                        serde_json::Value::String(v.to_str().unwrap_or("").to_string()),
                    )
                })
                .collect();

            let body = response
                .json::<serde_json::Value>()
                .await
                .unwrap_or(serde_json::Value::Null);

            Ok(NodeOutput::new()
                .with("response", body)
                .with("status", status)
                .with("headers", headers))
        })
    }
}

/// If node - conditional branching
pub struct IfNode;

impl NodeDefinition for IfNode {
    fn node_type(&self) -> &str {
        "if"
    }

    fn display_name(&self) -> &str {
        "If"
    }

    fn description(&self) -> &str {
        "Branches execution based on a condition"
    }

    fn category(&self) -> &str {
        "Logic"
    }

    fn inputs(&self) -> Vec<PinDefinition> {
        vec![
            PinDefinition::new("trigger", PinType::Trigger),
            PinDefinition::new("value", PinType::Any),
        ]
    }

    fn outputs(&self) -> Vec<PinDefinition> {
        vec![
            PinDefinition::new("true", PinType::Any),
            PinDefinition::new("false", PinType::Any),
        ]
    }

    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "condition": {
                    "type": "string",
                    "enum": ["equals", "not_equals", "greater", "less", "contains", "is_empty", "is_not_empty"],
                    "default": "equals"
                },
                "compare_value": { "type": "string" }
            }
        })
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a ExecutionContext,
    ) -> BoxFuture<'a, Result<NodeOutput, ExecutionError>> {
        Box::pin(async move {
            let value = ctx.get_input::<serde_json::Value>("value")?;
            let condition: String = ctx.get_config("condition").unwrap_or_else(|_| "is_not_empty".to_string());
            let compare_value = ctx.get_config::<serde_json::Value>("compare_value").ok();

            let result = match condition.as_str() {
                "equals" => compare_value.map(|cv| value == cv).unwrap_or(false),
                "not_equals" => compare_value.map(|cv| value != cv).unwrap_or(true),
                "is_empty" => match &value {
                    serde_json::Value::Null => true,
                    serde_json::Value::String(s) => s.is_empty(),
                    serde_json::Value::Array(a) => a.is_empty(),
                    serde_json::Value::Object(o) => o.is_empty(),
                    _ => false,
                },
                "is_not_empty" => match &value {
                    serde_json::Value::Null => false,
                    serde_json::Value::String(s) => !s.is_empty(),
                    serde_json::Value::Array(a) => !a.is_empty(),
                    serde_json::Value::Object(o) => !o.is_empty(),
                    _ => true,
                },
                "contains" => {
                    if let (serde_json::Value::String(haystack), Some(serde_json::Value::String(needle))) = (&value, &compare_value) {
                        haystack.contains(needle)
                    } else {
                        false
                    }
                },
                _ => false,
            };

            let mut output = NodeOutput::new();
            if result {
                output = output.with("true", value);
            } else {
                output = output.with("false", value);
            }

            Ok(output)
        })
    }
}

/// Set node - transforms or sets data
pub struct SetNode;

impl NodeDefinition for SetNode {
    fn node_type(&self) -> &str {
        "set"
    }

    fn display_name(&self) -> &str {
        "Set"
    }

    fn description(&self) -> &str {
        "Sets or transforms data values"
    }

    fn category(&self) -> &str {
        "Data"
    }

    fn inputs(&self) -> Vec<PinDefinition> {
        vec![
            PinDefinition::new("trigger", PinType::Trigger),
            PinDefinition::new("input", PinType::Any).optional(),
        ]
    }

    fn outputs(&self) -> Vec<PinDefinition> {
        vec![PinDefinition::new("output", PinType::Json)]
    }

    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "values": {
                    "type": "object",
                    "description": "Key-value pairs to set"
                }
            }
        })
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a ExecutionContext,
    ) -> BoxFuture<'a, Result<NodeOutput, ExecutionError>> {
        Box::pin(async move {
            let mut result = ctx
                .get_input_optional::<serde_json::Value>("input")
                .unwrap_or(serde_json::Value::Object(Default::default()));

            // Merge configured values
            if let Ok(values) = ctx.get_config::<serde_json::Map<String, serde_json::Value>>("values") {
                if let serde_json::Value::Object(ref mut obj) = result {
                    for (k, v) in values {
                        obj.insert(k, v);
                    }
                }
            }

            Ok(NodeOutput::new().with("output", result))
        })
    }
}

/// Creates a default node registry with all built-in nodes
pub fn create_default_registry() -> ngwa_executor::NodeRegistry {
    let mut registry = ngwa_executor::NodeRegistry::new();

    registry.register(ManualTriggerNode);
    registry.register(HttpRequestNode);
    registry.register(IfNode);
    registry.register(SetNode);

    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_definitions() {
        let manual = ManualTriggerNode;
        assert_eq!(manual.node_type(), "manual_trigger");
        assert_eq!(manual.category(), "Triggers");

        let http = HttpRequestNode;
        assert_eq!(http.node_type(), "http_request");
        assert!(!http.inputs().is_empty());
        assert!(!http.outputs().is_empty());
    }
}
