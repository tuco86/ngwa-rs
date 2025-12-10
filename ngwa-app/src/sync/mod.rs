//! High-frequency synchronization manager for SpacetimeDB.
//!
//! This module provides a SyncManager that coalesces high-frequency updates
//! (cursor movements, drag updates, physics vertices) into batched requests
//! sent every ~16ms to minimize network overhead.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Coalesced vertex data for physics sync.
#[derive(Debug, Clone)]
pub struct VertexData {
    pub index: u16,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}

impl VertexData {
    pub fn new(index: u16, x: f32, y: f32, vx: f32, vy: f32) -> Self {
        Self { index, x, y, vx, vy }
    }

    /// Serialize to JSON object string.
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"i":{},"x":{},"y":{},"vx":{},"vy":{}}}"#,
            self.index, self.x, self.y, self.vx, self.vy
        )
    }
}

/// Pending drag update.
#[derive(Debug, Clone)]
pub struct DragUpdate {
    pub x: f32,
    pub y: f32,
    pub sequence: u64,
}

/// High-frequency sync manager.
///
/// Coalesces updates and sends them in batches to reduce network traffic.
/// Designed to be called every frame, but only sends updates every `send_interval`.
pub struct SyncManager {
    /// Pending cursor position (only latest is kept).
    pending_cursor: Option<(f32, f32)>,

    /// Pending drag update (only latest is kept).
    pending_drag: Option<DragUpdate>,

    /// Pending edge vertex updates (keyed by edge_id, only latest per edge).
    pending_edge_vertices: HashMap<u64, Vec<VertexData>>,

    /// Sequence counter for ordering.
    sequence_counter: u64,

    /// Last time we sent an update.
    last_send: Instant,

    /// Minimum interval between sends (default 16ms for ~60fps).
    pub send_interval: Duration,

    /// Current workflow ID.
    workflow_id: String,

    /// Whether this client owns physics simulation.
    pub physics_owner: bool,

    /// Connection state.
    pub connected: bool,
}

impl Default for SyncManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncManager {
    /// Create a new SyncManager with default 16ms send interval.
    pub fn new() -> Self {
        Self {
            pending_cursor: None,
            pending_drag: None,
            pending_edge_vertices: HashMap::new(),
            sequence_counter: 0,
            last_send: Instant::now(),
            send_interval: Duration::from_millis(16),
            workflow_id: String::new(),
            physics_owner: false,
            connected: false,
        }
    }

    /// Set the current workflow ID.
    pub fn set_workflow(&mut self, workflow_id: String) {
        self.workflow_id = workflow_id;
    }

    /// Get the current workflow ID.
    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }

    /// Queue a cursor position update.
    pub fn queue_cursor(&mut self, x: f32, y: f32) {
        self.pending_cursor = Some((x, y));
    }

    /// Queue a drag position update.
    pub fn queue_drag(&mut self, x: f32, y: f32) {
        self.sequence_counter += 1;
        self.pending_drag = Some(DragUpdate {
            x,
            y,
            sequence: self.sequence_counter,
        });
    }

    /// Queue edge vertex updates (replaces any pending for this edge).
    pub fn queue_edge_vertices(&mut self, edge_id: u64, vertices: Vec<VertexData>) {
        self.pending_edge_vertices.insert(edge_id, vertices);
    }

    /// Check if there are any pending updates.
    pub fn has_pending(&self) -> bool {
        self.pending_cursor.is_some()
            || self.pending_drag.is_some()
            || !self.pending_edge_vertices.is_empty()
    }

    /// Check if enough time has passed to send updates.
    pub fn should_send(&self) -> bool {
        self.last_send.elapsed() >= self.send_interval && self.has_pending()
    }

    /// Drain all pending updates and return them for sending.
    ///
    /// Returns a list of (update_type, x, y, sequence, edge_id, vertices_json) tuples.
    /// update_type: 0 = cursor, 1 = drag, 2 = edge_vertices
    pub fn drain_pending(&mut self) -> Vec<BatchUpdateData> {
        if !self.should_send() {
            return Vec::new();
        }

        let mut updates = Vec::new();

        // Cursor update
        if let Some((x, y)) = self.pending_cursor.take() {
            updates.push(BatchUpdateData {
                update_type: 0,
                x,
                y,
                sequence: 0,
                edge_id: 0,
                vertices_json: String::new(),
            });
        }

        // Drag update
        if let Some(drag) = self.pending_drag.take() {
            updates.push(BatchUpdateData {
                update_type: 1,
                x: drag.x,
                y: drag.y,
                sequence: drag.sequence,
                edge_id: 0,
                vertices_json: String::new(),
            });
        }

        // Edge vertices updates (only if we own physics)
        if self.physics_owner {
            for (edge_id, vertices) in self.pending_edge_vertices.drain() {
                let json = format!(
                    "[{}]",
                    vertices.iter().map(|v| v.to_json()).collect::<Vec<_>>().join(",")
                );
                updates.push(BatchUpdateData {
                    update_type: 2,
                    x: 0.0,
                    y: 0.0,
                    sequence: 0,
                    edge_id,
                    vertices_json: json,
                });
            }
        } else {
            // Clear pending vertices if we don't own physics
            self.pending_edge_vertices.clear();
        }

        self.last_send = Instant::now();
        updates
    }

    /// Called each frame. Returns updates to send if interval has elapsed.
    pub fn tick(&mut self) -> Vec<BatchUpdateData> {
        self.drain_pending()
    }

    /// Clear all pending updates.
    pub fn clear(&mut self) {
        self.pending_cursor = None;
        self.pending_drag = None;
        self.pending_edge_vertices.clear();
    }

    /// Reset state (e.g., when disconnecting).
    pub fn reset(&mut self) {
        self.clear();
        self.workflow_id.clear();
        self.physics_owner = false;
        self.connected = false;
    }
}

/// Data for a single batch update.
#[derive(Debug, Clone)]
pub struct BatchUpdateData {
    pub update_type: u8,
    pub x: f32,
    pub y: f32,
    pub sequence: u64,
    pub edge_id: u64,
    pub vertices_json: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_manager_coalesces_cursor() {
        let mut manager = SyncManager::new();
        manager.send_interval = Duration::from_millis(0); // Immediate for testing

        manager.queue_cursor(1.0, 2.0);
        manager.queue_cursor(3.0, 4.0);
        manager.queue_cursor(5.0, 6.0);

        // Should only have the latest cursor position
        let updates = manager.tick();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].update_type, 0);
        assert_eq!(updates[0].x, 5.0);
        assert_eq!(updates[0].y, 6.0);
    }

    #[test]
    fn test_sync_manager_respects_interval() {
        let mut manager = SyncManager::new();
        manager.send_interval = Duration::from_secs(1); // Long interval

        manager.queue_cursor(1.0, 2.0);

        // Should not send yet
        let updates = manager.tick();
        assert!(updates.is_empty());

        // Still has pending
        assert!(manager.has_pending());
    }

    #[test]
    fn test_vertex_data_json() {
        let v = VertexData::new(5, 100.5, 200.5, 10.0, -5.0);
        let json = v.to_json();
        assert!(json.contains("\"i\":5"));
        assert!(json.contains("\"x\":100.5"));
    }

    #[test]
    fn test_physics_owner_gate() {
        let mut manager = SyncManager::new();
        manager.send_interval = Duration::from_millis(0);
        manager.physics_owner = false;

        manager.queue_edge_vertices(1, vec![VertexData::new(0, 0.0, 0.0, 0.0, 0.0)]);

        // Should clear vertices since we're not physics owner
        let updates = manager.tick();
        assert!(updates.is_empty());
    }

    #[test]
    fn test_physics_owner_sends_vertices() {
        let mut manager = SyncManager::new();
        manager.send_interval = Duration::from_millis(0);
        manager.physics_owner = true;

        manager.queue_edge_vertices(1, vec![
            VertexData::new(0, 0.0, 0.0, 0.0, 0.0),
            VertexData::new(1, 10.0, 10.0, 0.0, 0.0),
        ]);

        let updates = manager.tick();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].update_type, 2);
        assert_eq!(updates[0].edge_id, 1);
        assert!(updates[0].vertices_json.contains("\"i\":0"));
        assert!(updates[0].vertices_json.contains("\"i\":1"));
    }
}
