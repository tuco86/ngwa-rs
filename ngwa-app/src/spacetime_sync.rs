//! SpacetimeDB synchronization module
//!
//! Provides a thread-safe channel for SpacetimeDB callbacks to communicate
//! updates to the main Iced application thread.

use once_cell::sync::Lazy;
use std::sync::Mutex;

/// Database update events from SpacetimeDB callbacks
#[derive(Debug, Clone)]
pub enum DbUpdate {
    // Workflow updates
    WorkflowInserted { id: String, name: String },
    WorkflowUpdated { id: String, name: String },
    WorkflowDeleted { id: String },

    // Node updates
    NodeInserted {
        workflow_id: String,
        node_uuid: String,
        node_type: String,
        name: String,
        position_x: f32,
        position_y: f32,
    },
    NodeMoved {
        workflow_id: String,
        node_uuid: String,
        x: f32,
        y: f32,
    },
    NodeDeleted {
        workflow_id: String,
        node_uuid: String,
    },

    // Edge updates
    EdgeInserted {
        workflow_id: String,
        from_node_uuid: String,
        from_output: String,
        to_node_uuid: String,
        to_input: String,
    },
    EdgeDeleted {
        workflow_id: String,
        from_node_uuid: String,
        to_node_uuid: String,
    },
}

/// Thread-safe queue for updates from SpacetimeDB callbacks
pub static PENDING_UPDATES: Lazy<Mutex<Vec<DbUpdate>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Push an update to the queue (called from SpacetimeDB callbacks)
pub fn push_update(update: DbUpdate) {
    if let Ok(mut updates) = PENDING_UPDATES.lock() {
        updates.push(update);
    }
}

/// Drain all pending updates from the queue (called from main thread)
pub fn drain_updates() -> Vec<DbUpdate> {
    if let Ok(mut updates) = PENDING_UPDATES.lock() {
        std::mem::take(&mut *updates)
    } else {
        Vec::new()
    }
}
