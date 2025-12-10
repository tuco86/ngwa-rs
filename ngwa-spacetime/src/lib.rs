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
// COLLABORATION: USER SELECTION
// ============================================================================

#[spacetimedb::table(name = user_selection, public)]
pub struct UserSelection {
    #[primary_key]
    pub user_identity: Identity,
    #[index(btree)]
    pub workflow_id: String,
    pub selected_node_uuids: String, // JSON array ["uuid1", "uuid2"]
    pub last_updated: Timestamp,
}

// ============================================================================
// COLLABORATION: DRAG STATE
// ============================================================================

#[spacetimedb::table(name = drag_state, public)]
pub struct DragState {
    #[primary_key]
    pub user_identity: Identity,
    #[index(btree)]
    pub workflow_id: String,
    pub drag_type: String,        // "node", "group", "edge", "box_select"
    pub node_uuids: String,       // JSON array for affected nodes
    pub from_pin: String,         // For edge drag: "node_uuid:pin_id", empty otherwise
    pub start_x: f32,
    pub start_y: f32,
    pub current_x: f32,
    pub current_y: f32,
    pub last_updated: Timestamp,
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

// ============================================================================
// SELECTION REDUCERS
// ============================================================================

#[spacetimedb::reducer]
pub fn update_selection(ctx: &ReducerContext, workflow_id: String, node_uuids: String) {
    if let Some(mut selection) = ctx.db.user_selection().user_identity().find(ctx.sender) {
        selection.workflow_id = workflow_id;
        selection.selected_node_uuids = node_uuids;
        selection.last_updated = ctx.timestamp;
        ctx.db.user_selection().user_identity().update(selection);
    } else {
        ctx.db.user_selection().insert(UserSelection {
            user_identity: ctx.sender,
            workflow_id,
            selected_node_uuids: node_uuids,
            last_updated: ctx.timestamp,
        });
    }
}

#[spacetimedb::reducer]
pub fn clear_selection(ctx: &ReducerContext) {
    if let Some(selection) = ctx.db.user_selection().user_identity().find(ctx.sender) {
        ctx.db.user_selection().user_identity().delete(selection.user_identity);
    }
}

// ============================================================================
// DRAG STATE REDUCERS
// ============================================================================

#[spacetimedb::reducer]
pub fn start_drag(
    ctx: &ReducerContext,
    workflow_id: String,
    drag_type: String,
    node_uuids: String,
    from_pin: String,
    x: f32,
    y: f32,
) {
    // Remove any existing drag state for this user
    if let Some(existing) = ctx.db.drag_state().user_identity().find(ctx.sender) {
        ctx.db.drag_state().user_identity().delete(existing.user_identity);
    }

    ctx.db.drag_state().insert(DragState {
        user_identity: ctx.sender,
        workflow_id,
        drag_type,
        node_uuids,
        from_pin,
        start_x: x,
        start_y: y,
        current_x: x,
        current_y: y,
        last_updated: ctx.timestamp,
    });
}

#[spacetimedb::reducer]
pub fn update_drag(ctx: &ReducerContext, x: f32, y: f32) {
    if let Some(mut drag) = ctx.db.drag_state().user_identity().find(ctx.sender) {
        drag.current_x = x;
        drag.current_y = y;
        drag.last_updated = ctx.timestamp;
        ctx.db.drag_state().user_identity().update(drag);
    }
}

#[spacetimedb::reducer]
pub fn end_drag(ctx: &ReducerContext) {
    if let Some(drag) = ctx.db.drag_state().user_identity().find(ctx.sender) {
        ctx.db.drag_state().user_identity().delete(drag.user_identity);
    }
}

// ============================================================================
// PHYSICS: EDGE VERTEX TABLE
// ============================================================================

/// Edge vertex positions for physics wire simulation.
/// These are synced in real-time for multi-user collaboration.
#[spacetimedb::table(name = edge_vertex, public)]
pub struct EdgeVertex {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub workflow_id: String,
    #[index(btree)]
    pub edge_id: u64,            // References workflow_edge.id
    pub vertex_index: u16,        // Index within the edge (0 = start anchor)
    pub position_x: f32,
    pub position_y: f32,
    pub velocity_x: f32,
    pub velocity_y: f32,
    pub last_modified_at: Timestamp,
}

// ============================================================================
// PHYSICS: PHYSICS STATE TABLE
// ============================================================================

/// Physics simulation ownership per workflow.
/// Only one client at a time can run the physics simulation.
#[spacetimedb::table(name = physics_state, public)]
pub struct PhysicsState {
    #[primary_key]
    pub workflow_id: String,
    /// Identity of the client currently owning physics simulation (None = unclaimed)
    pub owner_identity: Option<Identity>,
    /// Whether physics simulation is currently running
    pub simulation_running: bool,
    /// Number of physics ticks processed
    pub tick_count: u64,
    pub last_tick_at: Timestamp,
}

// ============================================================================
// OBJECT LOCKING TABLE
// ============================================================================

/// Optimistic locking for nodes/edges to prevent edit conflicts.
/// Locks auto-expire after 5 seconds.
#[spacetimedb::table(name = object_lock, public)]
pub struct ObjectLock {
    #[primary_key]
    pub lock_key: String,           // "node:{uuid}" or "edge:{id}" or "vertex:{edge_id}:{index}"
    pub workflow_id: String,
    pub owner_identity: Identity,
    pub lock_type: u8,              // 0 = read, 1 = write
    pub acquired_at: Timestamp,
    pub expires_at: Timestamp,      // Auto-expire after 5s
}

// ============================================================================
// BATCH UPDATE FOR HIGH-FREQUENCY SYNC (16ms)
// ============================================================================

/// Single update in a batch (cursor, drag, or edge vertices).
#[derive(spacetimedb::SpacetimeType, Clone)]
pub struct BatchUpdate {
    pub update_type: u8,            // 0 = cursor, 1 = drag, 2 = edge_vertices
    pub x: f32,
    pub y: f32,
    pub sequence: u64,              // For ordering
    pub edge_id: u64,               // For edge_vertices update
    pub vertices_json: String,      // JSON array of {index, x, y, vx, vy}
}

// ============================================================================
// PHYSICS REDUCERS
// ============================================================================

/// Claim physics ownership for a workflow.
#[spacetimedb::reducer]
pub fn claim_physics_ownership(ctx: &ReducerContext, workflow_id: String) {
    if let Some(mut state) = ctx.db.physics_state().workflow_id().find(&workflow_id) {
        // Only claim if unclaimed or if we already own it
        if state.owner_identity.is_none() || state.owner_identity == Some(ctx.sender) {
            state.owner_identity = Some(ctx.sender);
            state.simulation_running = true;
            state.last_tick_at = ctx.timestamp;
            ctx.db.physics_state().workflow_id().update(state);
            log::info!("Physics ownership claimed for workflow {} by {:?}", workflow_id, ctx.sender);
        }
    } else {
        // Create new physics state
        ctx.db.physics_state().insert(PhysicsState {
            workflow_id: workflow_id.clone(),
            owner_identity: Some(ctx.sender),
            simulation_running: true,
            tick_count: 0,
            last_tick_at: ctx.timestamp,
        });
        log::info!("Physics state created for workflow {}", workflow_id);
    }
}

/// Release physics ownership.
#[spacetimedb::reducer]
pub fn release_physics_ownership(ctx: &ReducerContext, workflow_id: String) {
    if let Some(mut state) = ctx.db.physics_state().workflow_id().find(&workflow_id) {
        if state.owner_identity == Some(ctx.sender) {
            state.owner_identity = None;
            state.simulation_running = false;
            ctx.db.physics_state().workflow_id().update(state);
            log::info!("Physics ownership released for workflow {}", workflow_id);
        }
    }
}

/// Update edge vertex positions (called by physics owner).
#[spacetimedb::reducer]
pub fn update_edge_vertices(
    ctx: &ReducerContext,
    workflow_id: String,
    edge_id: u64,
    vertices_json: String, // JSON: [{index, x, y, vx, vy}, ...]
) {
    // Verify caller owns physics
    if let Some(state) = ctx.db.physics_state().workflow_id().find(&workflow_id) {
        if state.owner_identity != Some(ctx.sender) {
            log::warn!("Non-owner tried to update edge vertices");
            return;
        }
    }

    // Parse vertices and update
    // For simplicity, we'll use a basic JSON parsing approach
    // In production, use serde_json
    let now = ctx.timestamp;

    // Delete existing vertices for this edge
    let existing: Vec<_> = ctx.db.edge_vertex().edge_id().filter(&edge_id).collect();
    for v in existing {
        ctx.db.edge_vertex().id().delete(v.id);
    }

    // Parse and insert new vertices (simplified - expects format like "[{...},{...}]")
    // Note: In production, use proper serde_json deserialization
    if !vertices_json.is_empty() && vertices_json != "[]" {
        // This is a simplified parser - in real code use serde_json
        let trimmed = vertices_json.trim_start_matches('[').trim_end_matches(']');
        for entry in trimmed.split("},{") {
            let clean = entry.trim_start_matches('{').trim_end_matches('}');
            // Parse fields (very simplified)
            let mut index: u16 = 0;
            let mut px: f32 = 0.0;
            let mut py: f32 = 0.0;
            let mut vx: f32 = 0.0;
            let mut vy: f32 = 0.0;

            for field in clean.split(',') {
                let parts: Vec<&str> = field.split(':').collect();
                if parts.len() == 2 {
                    let key = parts[0].trim().trim_matches('"');
                    let val = parts[1].trim();
                    match key {
                        "index" | "i" => index = val.parse().unwrap_or(0),
                        "x" => px = val.parse().unwrap_or(0.0),
                        "y" => py = val.parse().unwrap_or(0.0),
                        "vx" => vx = val.parse().unwrap_or(0.0),
                        "vy" => vy = val.parse().unwrap_or(0.0),
                        _ => {}
                    }
                }
            }

            ctx.db.edge_vertex().insert(EdgeVertex {
                id: 0,
                workflow_id: workflow_id.clone(),
                edge_id,
                vertex_index: index,
                position_x: px,
                position_y: py,
                velocity_x: vx,
                velocity_y: vy,
                last_modified_at: now,
            });
        }
    }
}

/// Initialize edge vertices when an edge is created.
#[spacetimedb::reducer]
pub fn init_edge_vertices(
    ctx: &ReducerContext,
    workflow_id: String,
    edge_id: u64,
    vertex_count: u16,
    start_x: f32,
    start_y: f32,
    end_x: f32,
    end_y: f32,
) {
    let now = ctx.timestamp;

    for i in 0..vertex_count {
        let t = i as f32 / (vertex_count - 1).max(1) as f32;
        let x = start_x + (end_x - start_x) * t;
        let y = start_y + (end_y - start_y) * t;

        ctx.db.edge_vertex().insert(EdgeVertex {
            id: 0,
            workflow_id: workflow_id.clone(),
            edge_id,
            vertex_index: i,
            position_x: x,
            position_y: y,
            velocity_x: 0.0,
            velocity_y: 0.0,
            last_modified_at: now,
        });
    }
}

// ============================================================================
// OBJECT LOCKING REDUCERS
// ============================================================================

/// Try to acquire a lock on an object.
/// Returns silently - client should check lock table for success.
#[spacetimedb::reducer]
pub fn try_acquire_lock(
    ctx: &ReducerContext,
    workflow_id: String,
    lock_key: String,
    lock_type: u8,
    duration_ms: u64,
) {
    let now = ctx.timestamp;
    let expires = Timestamp::from_micros_since_unix_epoch(
        now.to_micros_since_unix_epoch() + (duration_ms as i64 * 1000)
    );

    // Check if lock exists and is still valid
    if let Some(existing) = ctx.db.object_lock().lock_key().find(&lock_key) {
        // Lock exists - check if expired
        if existing.expires_at.to_micros_since_unix_epoch() > now.to_micros_since_unix_epoch() {
            // Not expired - can only refresh if we own it
            if existing.owner_identity == ctx.sender {
                let mut lock = existing;
                lock.expires_at = expires;
                lock.lock_type = lock_type;
                ctx.db.object_lock().lock_key().update(lock);
            }
            // Otherwise lock acquisition fails (caller checks table)
            return;
        }
        // Expired - delete and allow new lock
        ctx.db.object_lock().lock_key().delete(existing.lock_key);
    }

    // Create new lock
    ctx.db.object_lock().insert(ObjectLock {
        lock_key,
        workflow_id,
        owner_identity: ctx.sender,
        lock_type,
        acquired_at: now,
        expires_at: expires,
    });
}

/// Release a lock.
#[spacetimedb::reducer]
pub fn release_lock(ctx: &ReducerContext, lock_key: String) {
    if let Some(lock) = ctx.db.object_lock().lock_key().find(&lock_key) {
        if lock.owner_identity == ctx.sender {
            ctx.db.object_lock().lock_key().delete(lock.lock_key);
        }
    }
}

// ============================================================================
// BATCH HIGH-FREQUENCY UPDATE REDUCER (16ms)
// ============================================================================

/// Process a batch of high-frequency updates.
/// Designed to be called every ~16ms with coalesced updates.
#[spacetimedb::reducer]
pub fn batch_high_freq_update(
    ctx: &ReducerContext,
    workflow_id: String,
    updates: Vec<BatchUpdate>,
) {
    for update in updates {
        match update.update_type {
            // Cursor update
            0 => {
                if let Some(mut presence) = ctx.db.user_presence().user_identity().find(ctx.sender) {
                    presence.cursor_x = update.x;
                    presence.cursor_y = update.y;
                    presence.last_seen = ctx.timestamp;
                    ctx.db.user_presence().user_identity().update(presence);
                }
            }
            // Drag update
            1 => {
                if let Some(mut drag) = ctx.db.drag_state().user_identity().find(ctx.sender) {
                    drag.current_x = update.x;
                    drag.current_y = update.y;
                    drag.last_updated = ctx.timestamp;
                    ctx.db.drag_state().user_identity().update(drag);
                }
            }
            // Edge vertices update
            2 => {
                // Verify physics ownership
                if let Some(state) = ctx.db.physics_state().workflow_id().find(&workflow_id) {
                    if state.owner_identity == Some(ctx.sender) {
                        // Process vertices update
                        // (reuse update_edge_vertices logic)
                        let _ = (update.edge_id, update.vertices_json);
                        // Note: In production, call internal function
                    }
                }
            }
            _ => {}
        }
    }
}
