//! ngwa-scheduler: Cron scheduling and webhook handling
//!
//! This crate provides:
//! - Cron-based workflow scheduling
//! - Webhook server for triggering workflows via HTTP

use axum::extract::{Json, Path, State};
use ngwa_core::{TriggerData, TriggerType, Workflow};
use ngwa_executor::WorkflowExecutor;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Manages cron-scheduled workflows
pub struct CronScheduler {
    jobs: RwLock<HashMap<Uuid, CronJob>>,
    executor: Arc<WorkflowExecutor>,
}

struct CronJob {
    workflow_id: Uuid,
    cron_expression: String,
    enabled: bool,
}

impl CronScheduler {
    pub fn new(executor: Arc<WorkflowExecutor>) -> Self {
        Self {
            jobs: RwLock::new(HashMap::new()),
            executor,
        }
    }

    /// Schedule a workflow to run on a cron schedule
    pub async fn schedule(
        &self,
        workflow_id: Uuid,
        cron_expression: impl Into<String>,
    ) -> Result<(), SchedulerError> {
        let expr = cron_expression.into();

        // Validate cron expression
        cron::Schedule::from_str(&expr)
            .map_err(|e| SchedulerError::InvalidCronExpression(e.to_string()))?;

        let job = CronJob {
            workflow_id,
            cron_expression: expr,
            enabled: true,
        };

        self.jobs.write().await.insert(workflow_id, job);
        Ok(())
    }

    /// Remove a scheduled workflow
    pub async fn unschedule(&self, workflow_id: Uuid) {
        self.jobs.write().await.remove(&workflow_id);
    }

    /// Enable or disable a scheduled job
    pub async fn set_enabled(&self, workflow_id: Uuid, enabled: bool) {
        if let Some(job) = self.jobs.write().await.get_mut(&workflow_id) {
            job.enabled = enabled;
        }
    }
}

/// Errors that can occur in the scheduler
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("Invalid cron expression: {0}")]
    InvalidCronExpression(String),

    #[error("Workflow not found: {0}")]
    WorkflowNotFound(Uuid),

    #[error("Scheduler error: {0}")]
    Internal(String),
}

use std::str::FromStr;

/// Webhook server configuration
pub struct WebhookServer {
    executor: Arc<WorkflowExecutor>,
    workflows: Arc<RwLock<HashMap<Uuid, Workflow>>>,
}

impl WebhookServer {
    pub fn new(
        executor: Arc<WorkflowExecutor>,
        workflows: Arc<RwLock<HashMap<Uuid, Workflow>>>,
    ) -> Self {
        Self {
            executor,
            workflows,
        }
    }

    /// Start the webhook server on the given port
    pub async fn start(self, port: u16) -> Result<(), SchedulerError> {
        use axum::{routing::post, Router};

        let app = Router::new()
            .route("/webhook/:workflow_id", post(handle_webhook))
            .with_state(Arc::new(self));

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));

        tracing::info!("Starting webhook server on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| SchedulerError::Internal(e.to_string()))?;

        axum::serve(listener, app)
            .await
            .map_err(|e| SchedulerError::Internal(e.to_string()))?;

        Ok(())
    }
}

async fn handle_webhook(
    Path(workflow_id): axum::extract::Path<Uuid>,
    State(server): axum::extract::State<Arc<WebhookServer>>,
    Json(body): axum::extract::Json<serde_json::Value>,
) -> Result<axum::extract::Json<serde_json::Value>, axum::http::StatusCode> {
    let workflows = server.workflows.read().await;
    let workflow = workflows
        .get(&workflow_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let trigger_data = TriggerData {
        trigger_type: TriggerType::Webhook,
        data: body,
    };

    let result = server
        .executor
        .execute(workflow, trigger_data)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(axum::extract::Json(serde_json::json!({
        "success": result.success,
        "workflow_id": result.workflow_id.to_string(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_validation() {
        // Valid cron expressions
        assert!(cron::Schedule::from_str("0 * * * * *").is_ok());
        assert!(cron::Schedule::from_str("0 0 * * * *").is_ok());

        // Invalid cron expressions
        assert!(cron::Schedule::from_str("invalid").is_err());
    }
}
