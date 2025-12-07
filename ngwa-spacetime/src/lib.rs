//! ngwa-spacetime: SpacetimeDB module for real-time workflow collaboration
//!
//! This module handles:
//! - Workflow storage and synchronization
//! - Real-time collaborative editing
//! - User presence and cursor positions
//! - Execution history logging

use spacetimedb::{Identity, ReducerContext, Table, Timestamp};

// ============================================================================
// WORKFLOW TABLE
// ============================================================================

#[spacetimedb::table(name = workflow, public)]
pub struct Workflow {
    #[primary_key]
    pub id: String, // UUID as string
    pub name: String,
    pub owner_identity: Identity,
    pub is_shared: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

// ============================================================================
// WORKFLOW NODE TABLE
// ============================================================================

#[spacetimedb::table(name = workflow_node, public)]
pub struct WorkflowNode {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub workflow_id: String,
    pub node_uuid: String, // Client-side UUID for this node
    pub node_type: String, // "http_request", "manual_trigger", etc.
    pub name: String,
    pub position_x: f32,
    pub position_y: f32,
    pub config_json: String, // Serialized node configuration
    pub disabled: bool,
    pub last_modified_by: Identity,
    pub last_modified_at: Timestamp,
}

// ============================================================================
// WORKFLOW EDGE TABLE
// ============================================================================

#[spacetimedb::table(name = workflow_edge, public)]
pub struct WorkflowEdge {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub workflow_id: String,
    pub from_node_uuid: String,
    pub from_output: String, // Output pin name
    pub to_node_uuid: String,
    pub to_input: String, // Input pin name
}

// ============================================================================
// EXECUTION LOG TABLE
// ============================================================================

#[spacetimedb::table(name = execution_log, public)]
pub struct ExecutionLog {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub workflow_id: String,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub status: String,       // "running", "success", "error"
    pub trigger_type: String, // "manual", "cron", "webhook"
    pub error_message: Option<String>,
    pub triggered_by: Identity,
}

// ============================================================================
// COLLABORATION: USER PRESENCE
// ============================================================================

#[spacetimedb::table(name = user_presence, public)]
pub struct UserPresence {
    #[primary_key]
    pub user_identity: Identity,
    pub workflow_id: String,
    pub nickname: String,
    pub cursor_color: u32, // RGBA packed as u32
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub last_seen: Timestamp,
}

// ============================================================================
// INIT REDUCER
// ============================================================================

#[spacetimedb::reducer(init)]
pub fn init(_ctx: &ReducerContext) {
    log::info!("ngwa-spacetime module initialized");
}

// ============================================================================
// CLIENT CONNECTION REDUCERS
// ============================================================================

#[spacetimedb::reducer(client_connected)]
pub fn identity_connected(ctx: &ReducerContext) {
    log::info!("Client connected: {:?}", ctx.sender);
}

#[spacetimedb::reducer(client_disconnected)]
pub fn identity_disconnected(ctx: &ReducerContext) {
    // Remove user presence when they disconnect
    if let Some(presence) = ctx.db.user_presence().user_identity().find(ctx.sender) {
        ctx.db.user_presence().user_identity().delete(presence.user_identity);
        log::info!("Removed presence for disconnected client: {:?}", ctx.sender);
    }
}

// ============================================================================
// WORKFLOW CRUD REDUCERS
// ============================================================================

#[spacetimedb::reducer]
pub fn create_workflow(ctx: &ReducerContext, id: String, name: String) {
    let now = ctx.timestamp;
    ctx.db.workflow().insert(Workflow {
        id: id.clone(),
        name,
        owner_identity: ctx.sender,
        is_shared: false,
        created_at: now,
        updated_at: now,
    });
    log::info!("Created workflow: {}", id);
}

#[spacetimedb::reducer]
pub fn update_workflow_name(ctx: &ReducerContext, workflow_id: String, name: String) {
    if let Some(mut workflow) = ctx.db.workflow().id().find(&workflow_id) {
        workflow.name = name;
        workflow.updated_at = ctx.timestamp;
        ctx.db.workflow().id().update(workflow);
    }
}

#[spacetimedb::reducer]
pub fn delete_workflow(ctx: &ReducerContext, workflow_id: String) {
    // Delete all nodes
    let nodes: Vec<_> = ctx
        .db
        .workflow_node()
        .workflow_id()
        .filter(&workflow_id)
        .collect();
    for node in nodes {
        ctx.db.workflow_node().id().delete(node.id);
    }

    // Delete all edges
    let edges: Vec<_> = ctx
        .db
        .workflow_edge()
        .workflow_id()
        .filter(&workflow_id)
        .collect();
    for edge in edges {
        ctx.db.workflow_edge().id().delete(edge.id);
    }

    // Delete workflow
    if ctx.db.workflow().id().find(&workflow_id).is_some() {
        ctx.db.workflow().id().delete(workflow_id.clone());
        log::info!("Deleted workflow: {}", workflow_id);
    }
}

#[spacetimedb::reducer]
pub fn share_workflow(ctx: &ReducerContext, workflow_id: String, is_shared: bool) {
    if let Some(mut workflow) = ctx.db.workflow().id().find(&workflow_id) {
        workflow.is_shared = is_shared;
        workflow.updated_at = ctx.timestamp;
        ctx.db.workflow().id().update(workflow);
    }
}

// ============================================================================
// NODE CRUD REDUCERS
// ============================================================================

#[spacetimedb::reducer]
pub fn add_node(
    ctx: &ReducerContext,
    workflow_id: String,
    node_uuid: String,
    node_type: String,
    name: String,
    position_x: f32,
    position_y: f32,
    config_json: String,
) {
    let now = ctx.timestamp;
    ctx.db.workflow_node().insert(WorkflowNode {
        id: 0, // auto_inc
        workflow_id: workflow_id.clone(),
        node_uuid: node_uuid.clone(),
        node_type,
        name,
        position_x,
        position_y,
        config_json,
        disabled: false,
        last_modified_by: ctx.sender,
        last_modified_at: now,
    });

    // Update workflow timestamp
    if let Some(mut workflow) = ctx.db.workflow().id().find(&workflow_id) {
        workflow.updated_at = now;
        ctx.db.workflow().id().update(workflow);
    }

    log::info!("Added node {} to workflow {}", node_uuid, workflow_id);
}

#[spacetimedb::reducer]
pub fn move_node(
    ctx: &ReducerContext,
    workflow_id: String,
    node_uuid: String,
    position_x: f32,
    position_y: f32,
) {
    let nodes: Vec<_> = ctx
        .db
        .workflow_node()
        .workflow_id()
        .filter(&workflow_id)
        .collect();

    for mut node in nodes {
        if node.node_uuid == node_uuid {
            node.position_x = position_x;
            node.position_y = position_y;
            node.last_modified_by = ctx.sender;
            node.last_modified_at = ctx.timestamp;
            ctx.db.workflow_node().id().update(node);
            break;
        }
    }
}

#[spacetimedb::reducer]
pub fn update_node_config(
    ctx: &ReducerContext,
    workflow_id: String,
    node_uuid: String,
    config_json: String,
) {
    let nodes: Vec<_> = ctx
        .db
        .workflow_node()
        .workflow_id()
        .filter(&workflow_id)
        .collect();

    for mut node in nodes {
        if node.node_uuid == node_uuid {
            node.config_json = config_json;
            node.last_modified_by = ctx.sender;
            node.last_modified_at = ctx.timestamp;
            ctx.db.workflow_node().id().update(node);
            break;
        }
    }
}

#[spacetimedb::reducer]
pub fn update_node_name(
    ctx: &ReducerContext,
    workflow_id: String,
    node_uuid: String,
    name: String,
) {
    let nodes: Vec<_> = ctx
        .db
        .workflow_node()
        .workflow_id()
        .filter(&workflow_id)
        .collect();

    for mut node in nodes {
        if node.node_uuid == node_uuid {
            node.name = name;
            node.last_modified_by = ctx.sender;
            node.last_modified_at = ctx.timestamp;
            ctx.db.workflow_node().id().update(node);
            break;
        }
    }
}

#[spacetimedb::reducer]
pub fn toggle_node_disabled(ctx: &ReducerContext, workflow_id: String, node_uuid: String) {
    let nodes: Vec<_> = ctx
        .db
        .workflow_node()
        .workflow_id()
        .filter(&workflow_id)
        .collect();

    for mut node in nodes {
        if node.node_uuid == node_uuid {
            node.disabled = !node.disabled;
            node.last_modified_by = ctx.sender;
            node.last_modified_at = ctx.timestamp;
            ctx.db.workflow_node().id().update(node);
            break;
        }
    }
}

#[spacetimedb::reducer]
pub fn delete_node(ctx: &ReducerContext, workflow_id: String, node_uuid: String) {
    // Delete the node
    let nodes: Vec<_> = ctx
        .db
        .workflow_node()
        .workflow_id()
        .filter(&workflow_id)
        .collect();

    for node in nodes {
        if node.node_uuid == node_uuid {
            ctx.db.workflow_node().id().delete(node.id);
            break;
        }
    }

    // Delete all edges connected to this node
    let edges: Vec<_> = ctx
        .db
        .workflow_edge()
        .workflow_id()
        .filter(&workflow_id)
        .collect();

    for edge in edges {
        if edge.from_node_uuid == node_uuid || edge.to_node_uuid == node_uuid {
            ctx.db.workflow_edge().id().delete(edge.id);
        }
    }

    log::info!("Deleted node {} from workflow {}", node_uuid, workflow_id);
}

// ============================================================================
// EDGE CRUD REDUCERS
// ============================================================================

#[spacetimedb::reducer]
pub fn add_edge(
    ctx: &ReducerContext,
    workflow_id: String,
    from_node_uuid: String,
    from_output: String,
    to_node_uuid: String,
    to_input: String,
) {
    ctx.db.workflow_edge().insert(WorkflowEdge {
        id: 0, // auto_inc
        workflow_id: workflow_id.clone(),
        from_node_uuid,
        from_output,
        to_node_uuid,
        to_input,
    });

    // Update workflow timestamp
    if let Some(mut workflow) = ctx.db.workflow().id().find(&workflow_id) {
        workflow.updated_at = ctx.timestamp;
        ctx.db.workflow().id().update(workflow);
    }
}

#[spacetimedb::reducer]
pub fn delete_edge(ctx: &ReducerContext, edge_id: u64) {
    if ctx.db.workflow_edge().id().find(edge_id).is_some() {
        ctx.db.workflow_edge().id().delete(edge_id);
    }
}

#[spacetimedb::reducer]
pub fn delete_edge_by_pins(
    ctx: &ReducerContext,
    workflow_id: String,
    from_node_uuid: String,
    from_output: String,
    to_node_uuid: String,
    to_input: String,
) {
    let edges: Vec<_> = ctx
        .db
        .workflow_edge()
        .workflow_id()
        .filter(&workflow_id)
        .collect();

    for edge in edges {
        if edge.from_node_uuid == from_node_uuid
            && edge.from_output == from_output
            && edge.to_node_uuid == to_node_uuid
            && edge.to_input == to_input
        {
            ctx.db.workflow_edge().id().delete(edge.id);
            break;
        }
    }
}

// ============================================================================
// EXECUTION LOG REDUCERS
// ============================================================================

#[spacetimedb::reducer]
pub fn start_execution(
    ctx: &ReducerContext,
    workflow_id: String,
    trigger_type: String,
) {
    ctx.db.execution_log().insert(ExecutionLog {
        id: 0, // auto_inc
        workflow_id,
        started_at: ctx.timestamp,
        finished_at: None,
        status: "running".to_string(),
        trigger_type,
        error_message: None,
        triggered_by: ctx.sender,
    });
}

#[spacetimedb::reducer]
pub fn finish_execution(ctx: &ReducerContext, execution_id: u64, success: bool, error_message: Option<String>) {
    if let Some(mut log_entry) = ctx.db.execution_log().id().find(execution_id) {
        log_entry.finished_at = Some(ctx.timestamp);
        log_entry.status = if success { "success" } else { "error" }.to_string();
        log_entry.error_message = error_message;
        ctx.db.execution_log().id().update(log_entry);
    }
}

// ============================================================================
// COLLABORATION REDUCERS
// ============================================================================

#[spacetimedb::reducer]
pub fn join_workflow(ctx: &ReducerContext, workflow_id: String, nickname: String, cursor_color: u32) {
    // Remove any existing presence for this user
    if let Some(existing) = ctx.db.user_presence().user_identity().find(ctx.sender) {
        ctx.db.user_presence().user_identity().delete(existing.user_identity);
    }

    // Add new presence
    ctx.db.user_presence().insert(UserPresence {
        user_identity: ctx.sender,
        workflow_id,
        nickname,
        cursor_color,
        cursor_x: 0.0,
        cursor_y: 0.0,
        last_seen: ctx.timestamp,
    });
}

#[spacetimedb::reducer]
pub fn leave_workflow(ctx: &ReducerContext) {
    if let Some(presence) = ctx.db.user_presence().user_identity().find(ctx.sender) {
        ctx.db.user_presence().user_identity().delete(presence.user_identity);
    }
}

#[spacetimedb::reducer]
pub fn update_cursor(ctx: &ReducerContext, cursor_x: f32, cursor_y: f32) {
    if let Some(mut presence) = ctx.db.user_presence().user_identity().find(ctx.sender) {
        presence.cursor_x = cursor_x;
        presence.cursor_y = cursor_y;
        presence.last_seen = ctx.timestamp;
        ctx.db.user_presence().user_identity().update(presence);
    }
}

#[spacetimedb::reducer]
pub fn update_nickname(ctx: &ReducerContext, nickname: String) {
    if let Some(mut presence) = ctx.db.user_presence().user_identity().find(ctx.sender) {
        presence.nickname = nickname;
        ctx.db.user_presence().user_identity().update(presence);
    }
}
