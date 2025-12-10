//! ngwa-app: Desktop application for the ngwa workflow automation system
//!
//! This is the main entry point for the ngwa desktop application.
//! It provides a visual editor for creating and managing workflows.

mod spacetime_sync;
mod spacetimedb_client;
mod sync;

use iced::widget::{button, column, container, mouse_area, row, scrollable, stack, text, text_input};
use iced::{keyboard, window, Color, Element, Event, Length, Subscription, Task, Theme};
use iced_nodegraph::{
    node_graph, node_pin, DragInfo, NodeContentStyle, NodeGraph, PinDirection, PinReference, PinSide,
    node_title_bar,
};
use iced_palette::{
    Command, Shortcut, command, command_palette, find_matching_shortcut, focus_input,
    get_filtered_command_index, get_filtered_count, is_toggle_shortcut, navigate_down, navigate_up,
};
use ngwa_core::{Workflow, WorkflowNode};
use spacetimedb_client::{
    DbConnection,
    // Table access traits
    WorkflowTableAccess,
    WorkflowNodeTableAccess,
    WorkflowEdgeTableAccess,
    UserPresenceTableAccess,
    UserSelectionTableAccess,
    DragStateTableAccess,
    // Reducer traits (needed for calling reducers on conn.reducers)
    create_workflow_reducer::create_workflow,
    delete_workflow_reducer::delete_workflow,
    move_node_reducer::move_node,
    add_node_reducer::add_node,
    add_edge_reducer::add_edge,
    delete_edge_by_pins_reducer::delete_edge_by_pins,
    delete_node_reducer::delete_node,
    join_workflow_reducer::join_workflow,
    leave_workflow_reducer::leave_workflow,
    update_cursor_reducer::update_cursor,
    update_selection_reducer::update_selection,
    start_drag_reducer::start_drag,
    update_drag_reducer::update_drag,
    end_drag_reducer::end_drag,
};
use spacetimedb_sdk::{DbContext, Table, TableWithPrimaryKey};
use std::sync::OnceLock;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

const SPACETIMEDB_URI: &str = "http://127.0.0.1:3000";
const DATABASE_NAME: &str = "ngwa-spacetime";

/// Global connection holder (initialized once)
static DB_CONNECTION: OnceLock<DbConnection> = OnceLock::new();

/// Predefined collaboration colors (distinct and visible)
const COLLAB_COLORS: [u32; 12] = [
    0xFF5733, // Orange-red
    0x33FF57, // Green
    0x3357FF, // Blue
    0xFF33F5, // Magenta
    0xFFD700, // Gold
    0x00CED1, // Dark Turquoise
    0xFF6347, // Tomato
    0x7B68EE, // Medium Slate Blue
    0x32CD32, // Lime Green
    0xFF69B4, // Hot Pink
    0x00BFFF, // Deep Sky Blue
    0xFFA500, // Orange
];

/// Generate a random user color from the palette
fn generate_user_color() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as usize)
        .unwrap_or(0);
    COLLAB_COLORS[seed % COLLAB_COLORS.len()]
}

/// State for a remote collaborator
#[derive(Debug, Clone)]
struct RemoteUser {
    /// SpacetimeDB identity as hex string
    identity: String,
    /// Display nickname
    nickname: String,
    /// User's color (RGB as u32)
    color: u32,
    /// Cursor position in world space (None if not in same workflow)
    cursor: Option<iced::Point>,
    /// Currently selected node UUIDs
    selected_nodes: HashSet<String>,
    /// Current drag state
    drag_state: Option<RemoteDragState>,
}

/// Remote user's drag operation
#[derive(Debug, Clone)]
enum RemoteDragState {
    /// Dragging nodes
    Node {
        node_uuids: Vec<String>,
        current_x: f32,
        current_y: f32,
    },
    /// Dragging an edge from a pin
    Edge {
        from_pin: String,
        current_x: f32,
        current_y: f32,
    },
    /// Box selection
    BoxSelect {
        start_x: f32,
        start_y: f32,
        current_x: f32,
        current_y: f32,
    },
}

fn main() -> iced::Result {
    iced::application(NgwaApp::new, NgwaApp::update, NgwaApp::view)
        .subscription(NgwaApp::subscription)
        .theme(NgwaApp::theme)
        .title("ngwa - Workflow Automation")
        .run()
}

/// Get reference to the global DB connection if available
fn get_connection() -> Option<&'static DbConnection> {
    DB_CONNECTION.get()
}

/// Main application state
struct NgwaApp {
    /// Connection status
    connection_status: ConnectionStatus,

    /// Current view
    view: AppView,

    /// List of workflows (from SpacetimeDB)
    workflows: Vec<WorkflowSummary>,

    /// Currently open workflow
    current_workflow: Option<Workflow>,

    /// Current workflow ID (SpacetimeDB)
    current_workflow_id: Option<String>,

    /// Node positions (visual only)
    node_positions: Vec<iced::Point>,

    /// Edges in the graph
    edges: Vec<(PinReference, PinReference)>,

    /// Selected nodes
    selected_nodes: HashSet<usize>,

    /// New workflow name input
    new_workflow_name: String,

    /// Current theme
    current_theme: Theme,

    /// Command palette state
    command_palette_open: bool,
    command_input: String,
    palette_view: PaletteView,
    palette_selected_index: usize,

    // Collaboration state
    /// User's nickname for collaboration
    user_nickname: Option<String>,
    /// User's assigned color (RGB as u32)
    user_color: u32,
    /// Input for nickname dialog
    nickname_input: String,
    /// Whether the nickname dialog is open
    nickname_dialog_open: bool,
    /// Pending workflow ID to open after nickname is set
    pending_workflow_id: Option<String>,
    /// Our SpacetimeDB identity (hex string)
    my_identity: Option<String>,
    /// Remote users in the current workflow (keyed by identity hex)
    remote_users: HashMap<String, RemoteUser>,
}

/// Command palette view state
#[derive(Debug, Clone, PartialEq)]
enum PaletteView {
    Main,
    Submenu(String),
}

#[derive(Debug, Clone, Default)]
enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// Summary of a workflow for the list view
#[derive(Debug, Clone, PartialEq)]
struct WorkflowSummary {
    id: String,
    name: String,
}

/// Different views in the application
#[derive(Debug, Clone, Default)]
enum AppView {
    #[default]
    WorkflowList,
    WorkflowEditor,
}

/// Messages for the application
#[derive(Debug, Clone)]
enum Message {
    // Connection
    Connect,
    ConnectionSuccess,
    ConnectionError(String),
    ConnectionTick,

    // Navigation
    OpenWorkflowList,
    OpenWorkflowEditor(String),

    // Workflow management
    CreateWorkflow,
    DeleteWorkflow(String),
    NewWorkflowNameChanged(String),

    // Graph editing
    NodeMoved {
        node_id: usize,
        position: iced::Point,
    },
    EdgeConnected {
        from_node: usize,
        from_pin: usize,
        to_node: usize,
        to_pin: usize,
    },
    EdgeDisconnected {
        from_node: usize,
        from_pin: usize,
        to_node: usize,
        to_pin: usize,
    },
    SelectionChanged(Vec<usize>),
    DeleteNodes(Vec<usize>),
    GroupMoved {
        node_ids: Vec<usize>,
        delta: iced::Vector,
    },

    // Command palette
    ToggleCommandPalette,
    CommandPaletteInput(String),
    CommandPaletteNavigateUp,
    CommandPaletteNavigateDown,
    CommandPaletteSelect(usize),
    CommandPaletteConfirm,
    CommandPaletteCancel,
    CommandPaletteNavigate(usize),
    ExecuteShortcut(String),
    NavigateToSubmenu(String),
    NavigateBack,

    // Node spawning
    SpawnNode { node_type: String, name: String },

    // Theme
    ChangeTheme(Theme),

    // Nickname dialog
    NicknameInputChanged(String),
    NicknameConfirm,
    NicknameCancel,

    // Collaboration events
    CursorMoved { x: f32, y: f32 },
    DragStarted(iced_nodegraph::DragInfo),
    DragUpdated { x: f32, y: f32 },
    DragEnded,

    // Animation tick
    Tick,
}

impl NgwaApp {
    fn new() -> (Self, Task<Message>) {
        let app = Self {
            connection_status: ConnectionStatus::Disconnected,
            view: AppView::WorkflowList,
            workflows: Vec::new(),
            current_workflow: None,
            current_workflow_id: None,
            node_positions: Vec::new(),
            edges: Vec::new(),
            selected_nodes: HashSet::new(),
            new_workflow_name: String::new(),
            current_theme: Theme::CatppuccinFrappe,
            command_palette_open: false,
            command_input: String::new(),
            palette_view: PaletteView::Main,
            palette_selected_index: 0,
            // Collaboration state
            user_nickname: None,
            user_color: generate_user_color(),
            nickname_input: String::new(),
            nickname_dialog_open: false,
            pending_workflow_id: None,
            my_identity: None,
            remote_users: HashMap::new(),
        };

        // Connect to SpacetimeDB immediately
        (app, Task::done(Message::Connect))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Connect => {
                self.connection_status = ConnectionStatus::Connecting;

                return Task::perform(
                    async {
                        // Use tokio::task::spawn_blocking for the synchronous connection
                        // with a timeout to prevent hanging
                        let connection_future = tokio::task::spawn_blocking(|| {
                            DbConnection::builder()
                                .with_uri(SPACETIMEDB_URI)
                                .with_module_name(DATABASE_NAME)
                                .build()
                        });

                        // 5 second timeout for connection
                        let result = tokio::time::timeout(
                            tokio::time::Duration::from_secs(5),
                            connection_future,
                        )
                        .await
                        .map_err(|_| "Connection timeout (5s)".to_string())?
                        .map_err(|e| format!("Task join error: {}", e))?;

                        match result {
                            Ok(conn) => {
                                // Register callbacks BEFORE starting the connection thread
                                // These callbacks push updates to our thread-safe queue

                                // Workflow callbacks
                                conn.db.workflow().on_insert(|_ctx, workflow| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::WorkflowInserted {
                                        id: workflow.id.clone(),
                                        name: workflow.name.clone(),
                                    });
                                });

                                conn.db.workflow().on_delete(|_ctx, workflow| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::WorkflowDeleted {
                                        id: workflow.id.clone(),
                                    });
                                });

                                // Node callbacks
                                conn.db.workflow_node().on_insert(|_ctx, node| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::NodeInserted {
                                        workflow_id: node.workflow_id.clone(),
                                        node_uuid: node.node_uuid.clone(),
                                        node_type: node.node_type.clone(),
                                        name: node.name.clone(),
                                        position_x: node.position_x,
                                        position_y: node.position_y,
                                    });
                                });

                                conn.db.workflow_node().on_update(|_ctx, _old, node| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::NodeMoved {
                                        workflow_id: node.workflow_id.clone(),
                                        node_uuid: node.node_uuid.clone(),
                                        x: node.position_x,
                                        y: node.position_y,
                                    });
                                });

                                conn.db.workflow_node().on_delete(|_ctx, node| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::NodeDeleted {
                                        workflow_id: node.workflow_id.clone(),
                                        node_uuid: node.node_uuid.clone(),
                                    });
                                });

                                // Edge callbacks
                                conn.db.workflow_edge().on_insert(|_ctx, edge| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::EdgeInserted {
                                        workflow_id: edge.workflow_id.clone(),
                                        from_node_uuid: edge.from_node_uuid.clone(),
                                        from_output: edge.from_output.clone(),
                                        to_node_uuid: edge.to_node_uuid.clone(),
                                        to_input: edge.to_input.clone(),
                                    });
                                });

                                conn.db.workflow_edge().on_delete(|_ctx, edge| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::EdgeDeleted {
                                        workflow_id: edge.workflow_id.clone(),
                                        from_node_uuid: edge.from_node_uuid.clone(),
                                        to_node_uuid: edge.to_node_uuid.clone(),
                                    });
                                });

                                // User presence callbacks (collaboration)
                                conn.db.user_presence().on_insert(|_ctx, presence| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::UserJoined {
                                        identity: presence.user_identity.to_string(),
                                        workflow_id: presence.workflow_id.clone(),
                                        nickname: presence.nickname.clone(),
                                        color: presence.cursor_color,
                                    });
                                });

                                conn.db.user_presence().on_update(|_ctx, _old, presence| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::UserCursorMoved {
                                        identity: presence.user_identity.to_string(),
                                        workflow_id: presence.workflow_id.clone(),
                                        x: presence.cursor_x,
                                        y: presence.cursor_y,
                                    });
                                });

                                conn.db.user_presence().on_delete(|_ctx, presence| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::UserLeft {
                                        identity: presence.user_identity.to_string(),
                                    });
                                });

                                // User selection callbacks
                                conn.db.user_selection().on_insert(|_ctx, selection| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::UserSelectionChanged {
                                        identity: selection.user_identity.to_string(),
                                        workflow_id: selection.workflow_id.clone(),
                                        node_uuids: selection.selected_node_uuids.clone(),
                                    });
                                });

                                conn.db.user_selection().on_update(|_ctx, _old, selection| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::UserSelectionChanged {
                                        identity: selection.user_identity.to_string(),
                                        workflow_id: selection.workflow_id.clone(),
                                        node_uuids: selection.selected_node_uuids.clone(),
                                    });
                                });

                                // Drag state callbacks
                                conn.db.drag_state().on_insert(|_ctx, drag| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::UserDragStarted {
                                        identity: drag.user_identity.to_string(),
                                        workflow_id: drag.workflow_id.clone(),
                                        drag_type: drag.drag_type.clone(),
                                        node_uuids: drag.node_uuids.clone(),
                                        from_pin: drag.from_pin.clone(),
                                        start_x: drag.start_x,
                                        start_y: drag.start_y,
                                    });
                                });

                                conn.db.drag_state().on_update(|_ctx, _old, drag| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::UserDragUpdated {
                                        identity: drag.user_identity.to_string(),
                                        x: drag.current_x,
                                        y: drag.current_y,
                                    });
                                });

                                conn.db.drag_state().on_delete(|_ctx, drag| {
                                    spacetime_sync::push_update(spacetime_sync::DbUpdate::UserDragEnded {
                                        identity: drag.user_identity.to_string(),
                                    });
                                });

                                // Start the connection processing thread AFTER registering callbacks
                                conn.run_threaded();

                                // Subscribe to all tables including collaboration tables
                                conn.subscription_builder()
                                    .subscribe([
                                        "SELECT * FROM workflow",
                                        "SELECT * FROM workflow_node",
                                        "SELECT * FROM workflow_edge",
                                        "SELECT * FROM user_presence",
                                        "SELECT * FROM user_selection",
                                        "SELECT * FROM drag_state",
                                    ]);

                                // Store in global holder
                                let _ = DB_CONNECTION.set(conn);
                                Ok(())
                            }
                            Err(e) => Err(format!("Connection failed: {}", e)),
                        }
                    },
                    |result| match result {
                        Ok(()) => Message::ConnectionSuccess,
                        Err(e) => Message::ConnectionError(e),
                    },
                );
            }

            Message::ConnectionSuccess => {
                self.connection_status = ConnectionStatus::Connected;
                // Store our identity
                if let Some(conn) = get_connection() {
                    if let Some(identity) = conn.try_identity() {
                        self.my_identity = Some(identity.to_string());
                    }
                }
                // Start tick loop - initial data comes via on_insert callbacks
                return Task::done(Message::ConnectionTick);
            }

            Message::ConnectionError(e) => {
                self.connection_status = ConnectionStatus::Error(e);
            }

            Message::ConnectionTick => {
                // Process all pending updates from SpacetimeDB callbacks
                let updates = spacetime_sync::drain_updates();

                for update in updates {
                    match update {
                        spacetime_sync::DbUpdate::WorkflowInserted { id, name } => {
                            // Add to list if not already present
                            if !self.workflows.iter().any(|w| w.id == id) {
                                self.workflows.push(WorkflowSummary { id, name });
                            }
                        }
                        spacetime_sync::DbUpdate::WorkflowUpdated { id, name } => {
                            // Update existing workflow name
                            if let Some(w) = self.workflows.iter_mut().find(|w| w.id == id) {
                                w.name = name;
                            }
                        }
                        spacetime_sync::DbUpdate::WorkflowDeleted { id } => {
                            self.workflows.retain(|w| w.id != id);
                        }
                        spacetime_sync::DbUpdate::NodeMoved { workflow_id, node_uuid, x, y } => {
                            // Update node position if we're viewing this workflow
                            if self.current_workflow_id.as_ref() == Some(&workflow_id) {
                                if let Some(ref mut workflow) = self.current_workflow {
                                    // Find node index by UUID
                                    if let Some(idx) = workflow.nodes.iter()
                                        .position(|n| n.id.to_string() == node_uuid)
                                    {
                                        if idx < self.node_positions.len() {
                                            self.node_positions[idx] = iced::Point::new(x, y);
                                        }
                                        // Also update the workflow node position
                                        if let Some(node) = workflow.nodes.get_mut(idx) {
                                            node.position = (x, y);
                                        }
                                    }
                                }
                            }
                        }
                        spacetime_sync::DbUpdate::NodeInserted {
                            workflow_id,
                            node_uuid,
                            node_type,
                            name,
                            position_x,
                            position_y,
                        } => {
                            // Add node if we're viewing this workflow and it doesn't exist
                            if self.current_workflow_id.as_ref() == Some(&workflow_id) {
                                if let Some(ref mut workflow) = self.current_workflow {
                                    // Check if node already exists
                                    let exists = workflow.nodes.iter()
                                        .any(|n| n.id.to_string() == node_uuid);
                                    if !exists {
                                        let node_id = Uuid::parse_str(&node_uuid)
                                            .unwrap_or_else(|_| Uuid::new_v4());
                                        let node = WorkflowNode::new(&node_type, (position_x, position_y))
                                            .with_id(node_id)
                                            .with_name(&name);
                                        workflow.add_node(node);
                                        self.node_positions.push(iced::Point::new(position_x, position_y));
                                    }
                                }
                            }
                        }
                        spacetime_sync::DbUpdate::NodeDeleted { workflow_id, node_uuid } => {
                            // Remove node if we're viewing this workflow
                            if self.current_workflow_id.as_ref() == Some(&workflow_id) {
                                if let Some(ref mut workflow) = self.current_workflow {
                                    if let Some(idx) = workflow.nodes.iter()
                                        .position(|n| n.id.to_string() == node_uuid)
                                    {
                                        workflow.nodes.remove(idx);
                                        if idx < self.node_positions.len() {
                                            self.node_positions.remove(idx);
                                        }
                                        // Also remove edges connected to this node
                                        self.edges.retain(|(from, to)| {
                                            from.node_id != idx && to.node_id != idx
                                        });
                                        // Adjust edge indices for nodes after the removed one
                                        for (from, to) in &mut self.edges {
                                            if from.node_id > idx {
                                                from.node_id -= 1;
                                            }
                                            if to.node_id > idx {
                                                to.node_id -= 1;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        spacetime_sync::DbUpdate::EdgeInserted {
                            workflow_id,
                            from_node_uuid,
                            from_output: _,
                            to_node_uuid,
                            to_input: _,
                        } => {
                            // Add edge if we're viewing this workflow
                            if self.current_workflow_id.as_ref() == Some(&workflow_id) {
                                if let Some(workflow) = &self.current_workflow {
                                    let from_idx = workflow.nodes.iter()
                                        .position(|n| n.id.to_string() == from_node_uuid);
                                    let to_idx = workflow.nodes.iter()
                                        .position(|n| n.id.to_string() == to_node_uuid);

                                    if let (Some(from), Some(to)) = (from_idx, to_idx) {
                                        let edge = (
                                            PinReference::new(from, 0),
                                            PinReference::new(to, 0),
                                        );
                                        // Only add if not already present
                                        if !self.edges.contains(&edge) {
                                            self.edges.push(edge);
                                        }
                                    }
                                }
                            }
                        }
                        spacetime_sync::DbUpdate::EdgeDeleted {
                            workflow_id,
                            from_node_uuid,
                            to_node_uuid,
                        } => {
                            // Remove edge if we're viewing this workflow
                            if self.current_workflow_id.as_ref() == Some(&workflow_id) {
                                if let Some(workflow) = &self.current_workflow {
                                    let from_idx = workflow.nodes.iter()
                                        .position(|n| n.id.to_string() == from_node_uuid);
                                    let to_idx = workflow.nodes.iter()
                                        .position(|n| n.id.to_string() == to_node_uuid);

                                    if let (Some(from), Some(to)) = (from_idx, to_idx) {
                                        self.edges.retain(|(f, t)| {
                                            !(f.node_id == from && t.node_id == to)
                                        });
                                    }
                                }
                            }
                        }

                        // Collaboration: User joined
                        spacetime_sync::DbUpdate::UserJoined {
                            identity,
                            workflow_id,
                            nickname,
                            color,
                        } => {
                            // Only track users in our current workflow, and skip ourselves
                            if self.current_workflow_id.as_ref() == Some(&workflow_id)
                                && self.my_identity.as_ref() != Some(&identity)
                            {
                                self.remote_users.insert(
                                    identity.clone(),
                                    RemoteUser {
                                        identity,
                                        nickname,
                                        color,
                                        cursor: None,
                                        selected_nodes: HashSet::new(),
                                        drag_state: None,
                                    },
                                );
                            }
                        }

                        // Collaboration: User left
                        spacetime_sync::DbUpdate::UserLeft { identity } => {
                            self.remote_users.remove(&identity);
                        }

                        // Collaboration: Cursor moved
                        spacetime_sync::DbUpdate::UserCursorMoved {
                            identity,
                            workflow_id,
                            x,
                            y,
                        } => {
                            if self.current_workflow_id.as_ref() == Some(&workflow_id) {
                                if let Some(user) = self.remote_users.get_mut(&identity) {
                                    user.cursor = Some(iced::Point::new(x, y));
                                }
                            }
                        }

                        // Collaboration: Selection changed
                        spacetime_sync::DbUpdate::UserSelectionChanged {
                            identity,
                            workflow_id,
                            node_uuids,
                        } => {
                            if self.current_workflow_id.as_ref() == Some(&workflow_id) {
                                if let Some(user) = self.remote_users.get_mut(&identity) {
                                    // Parse JSON array
                                    if let Ok(uuids) = serde_json::from_str::<Vec<String>>(&node_uuids) {
                                        user.selected_nodes = uuids.into_iter().collect();
                                    }
                                }
                            }
                        }

                        // Collaboration: Drag started
                        spacetime_sync::DbUpdate::UserDragStarted {
                            identity,
                            workflow_id,
                            drag_type,
                            node_uuids,
                            from_pin,
                            start_x,
                            start_y,
                        } => {
                            if self.current_workflow_id.as_ref() == Some(&workflow_id) {
                                if let Some(user) = self.remote_users.get_mut(&identity) {
                                    user.drag_state = match drag_type.as_str() {
                                        "node" | "group" => {
                                            let uuids = serde_json::from_str::<Vec<String>>(&node_uuids)
                                                .unwrap_or_default();
                                            Some(RemoteDragState::Node {
                                                node_uuids: uuids,
                                                current_x: start_x,
                                                current_y: start_y,
                                            })
                                        }
                                        "edge" => Some(RemoteDragState::Edge {
                                            from_pin,
                                            current_x: start_x,
                                            current_y: start_y,
                                        }),
                                        "box_select" => Some(RemoteDragState::BoxSelect {
                                            start_x,
                                            start_y,
                                            current_x: start_x,
                                            current_y: start_y,
                                        }),
                                        _ => None,
                                    };
                                }
                            }
                        }

                        // Collaboration: Drag updated
                        spacetime_sync::DbUpdate::UserDragUpdated { identity, x, y } => {
                            if let Some(user) = self.remote_users.get_mut(&identity) {
                                match &mut user.drag_state {
                                    Some(RemoteDragState::Node { current_x, current_y, .. }) => {
                                        *current_x = x;
                                        *current_y = y;
                                    }
                                    Some(RemoteDragState::Edge { current_x, current_y, .. }) => {
                                        *current_x = x;
                                        *current_y = y;
                                    }
                                    Some(RemoteDragState::BoxSelect { current_x, current_y, .. }) => {
                                        *current_x = x;
                                        *current_y = y;
                                    }
                                    None => {}
                                }
                            }
                        }

                        // Collaboration: Drag ended
                        spacetime_sync::DbUpdate::UserDragEnded { identity } => {
                            if let Some(user) = self.remote_users.get_mut(&identity) {
                                user.drag_state = None;
                            }
                        }
                    }
                }

                // Schedule next tick (16ms for ~60fps responsiveness)
                if get_connection().is_some() {
                    return Task::perform(
                        async {
                            tokio::time::sleep(tokio::time::Duration::from_millis(16)).await;
                        },
                        |_| Message::ConnectionTick,
                    );
                }
            }

            Message::OpenWorkflowList => {
                // Leave the current workflow if we're in one
                if self.current_workflow_id.is_some() {
                    if let Some(conn) = get_connection() {
                        let _ = conn.reducers.leave_workflow();
                    }
                }

                self.view = AppView::WorkflowList;
                self.current_workflow = None;
                self.current_workflow_id = None;
                // Clear remote users when leaving a workflow
                self.remote_users.clear();
                // No need to sync - callbacks keep the list up to date
            }

            Message::OpenWorkflowEditor(id) => {
                // Check if nickname is set
                if self.user_nickname.is_none() {
                    // Show nickname dialog first
                    self.pending_workflow_id = Some(id);
                    self.nickname_dialog_open = true;
                    self.nickname_input.clear();
                    return Task::none();
                }

                // Proceed with opening the workflow
                self.current_workflow_id = Some(id.clone());

                // Load workflow data from SpacetimeDB cache
                if let Some(conn) = get_connection() {
                    // Find the workflow
                    if let Some(workflow_data) = conn.db.workflow().id().find(&id) {
                        let mut workflow = Workflow::new(&workflow_data.name);

                        // Load nodes
                        let nodes: Vec<_> = conn.db.workflow_node().iter()
                            .filter(|n| n.workflow_id == id)
                            .collect();

                        self.node_positions.clear();
                        for node_data in &nodes {
                            // Parse UUID from SpacetimeDB - use the stored node_uuid
                            let node_id = Uuid::parse_str(&node_data.node_uuid)
                                .unwrap_or_else(|_| Uuid::new_v4());
                            let node = WorkflowNode::new(&node_data.node_type, (node_data.position_x, node_data.position_y))
                                .with_id(node_id)
                                .with_name(&node_data.name);
                            workflow.add_node(node);
                            self.node_positions.push(iced::Point::new(node_data.position_x, node_data.position_y));
                        }

                        // Load edges
                        self.edges.clear();
                        let edges: Vec<_> = conn.db.workflow_edge().iter()
                            .filter(|e| e.workflow_id == id)
                            .collect();

                        for edge_data in &edges {
                            // Find node indices by UUID
                            let from_idx = nodes.iter().position(|n| n.node_uuid == edge_data.from_node_uuid);
                            let to_idx = nodes.iter().position(|n| n.node_uuid == edge_data.to_node_uuid);

                            if let (Some(from), Some(to)) = (from_idx, to_idx) {
                                // For now, use pin 0 (we'd need pin name mapping for proper support)
                                self.edges.push((
                                    PinReference::new(from, 0),
                                    PinReference::new(to, 0),
                                ));
                            }
                        }

                        self.current_workflow = Some(workflow);
                    }

                    // Join workflow for collaboration
                    if let Some(nickname) = &self.user_nickname {
                        let _ = conn.reducers.join_workflow(
                            id.clone(),
                            nickname.clone(),
                            self.user_color,
                        );
                    }

                    // Load existing users in this workflow
                    self.remote_users.clear();
                    for presence in conn.db.user_presence().iter() {
                        if presence.workflow_id == id {
                            let identity = presence.user_identity.to_string();
                            // Skip ourselves
                            if self.my_identity.as_ref() != Some(&identity) {
                                self.remote_users.insert(
                                    identity.clone(),
                                    RemoteUser {
                                        identity,
                                        nickname: presence.nickname.clone(),
                                        color: presence.cursor_color,
                                        cursor: Some(iced::Point::new(presence.cursor_x, presence.cursor_y)),
                                        selected_nodes: HashSet::new(),
                                        drag_state: None,
                                    },
                                );
                            }
                        }
                    }
                }

                self.view = AppView::WorkflowEditor;
            }

            Message::CreateWorkflow => {
                if !self.new_workflow_name.is_empty() {
                    let id = Uuid::new_v4().to_string();
                    let name = self.new_workflow_name.clone();
                    self.new_workflow_name.clear();

                    // Fire and forget - callback will update UI automatically
                    if let Some(conn) = get_connection() {
                        let _ = conn.reducers.create_workflow(id, name);
                    }
                }
            }

            Message::DeleteWorkflow(id) => {
                // Fire and forget - callback will update UI automatically
                if let Some(conn) = get_connection() {
                    let _ = conn.reducers.delete_workflow(id);
                }
            }

            Message::NewWorkflowNameChanged(name) => {
                self.new_workflow_name = name;
            }

            Message::NodeMoved { node_id, position } => {
                if node_id < self.node_positions.len() {
                    self.node_positions[node_id] = position;
                }
                if let Some(ref mut workflow) = self.current_workflow {
                    if let Some(node) = workflow.nodes.get_mut(node_id) {
                        node.position = (position.x, position.y);
                    }
                }

                // Fire and forget - server will echo back via callback
                if let (Some(workflow_id), Some(workflow)) = (self.current_workflow_id.clone(), &self.current_workflow) {
                    if let Some(node) = workflow.nodes.get(node_id) {
                        if let Some(conn) = get_connection() {
                            let _ = conn.reducers.move_node(
                                workflow_id,
                                node.id.to_string(),
                                position.x,
                                position.y,
                            );
                        }
                    }
                }
            }

            Message::GroupMoved { node_ids, delta } => {
                // Update all positions locally
                for &node_id in &node_ids {
                    if node_id < self.node_positions.len() {
                        self.node_positions[node_id].x += delta.x;
                        self.node_positions[node_id].y += delta.y;
                    }
                    if let Some(ref mut workflow) = self.current_workflow {
                        if let Some(node) = workflow.nodes.get_mut(node_id) {
                            node.position.0 += delta.x;
                            node.position.1 += delta.y;
                        }
                    }
                }

                // Sync to SpacetimeDB
                if let (Some(workflow_id), Some(workflow)) =
                    (&self.current_workflow_id, &self.current_workflow)
                {
                    if let Some(conn) = get_connection() {
                        for &node_id in &node_ids {
                            if let Some(node) = workflow.nodes.get(node_id) {
                                let _ = conn.reducers.move_node(
                                    workflow_id.clone(),
                                    node.id.to_string(),
                                    node.position.0,
                                    node.position.1,
                                );
                            }
                        }
                    }
                }
            }

            Message::EdgeConnected {
                from_node,
                from_pin,
                to_node,
                to_pin,
            } => {
                self.edges.push((
                    PinReference::new(from_node, from_pin),
                    PinReference::new(to_node, to_pin),
                ));

                // Fire and forget - server will echo back via callback
                if let (Some(workflow_id), Some(workflow)) = (self.current_workflow_id.clone(), &self.current_workflow) {
                    let from_uuid = workflow.nodes.get(from_node).map(|n| n.id.to_string());
                    let to_uuid = workflow.nodes.get(to_node).map(|n| n.id.to_string());

                    if let (Some(from_uuid), Some(to_uuid)) = (from_uuid, to_uuid) {
                        if let Some(conn) = get_connection() {
                            let _ = conn.reducers.add_edge(
                                workflow_id,
                                from_uuid,
                                format!("output_{}", from_pin),
                                to_uuid,
                                format!("input_{}", to_pin),
                            );
                        }
                    }
                }
            }

            Message::EdgeDisconnected {
                from_node,
                from_pin,
                to_node,
                to_pin,
            } => {
                self.edges.retain(|(from, to)| {
                    !(from.node_id == from_node
                        && from.pin_id == from_pin
                        && to.node_id == to_node
                        && to.pin_id == to_pin)
                });

                // Fire and forget - server will echo back via callback
                if let (Some(workflow_id), Some(workflow)) = (self.current_workflow_id.clone(), &self.current_workflow) {
                    let from_uuid = workflow.nodes.get(from_node).map(|n| n.id.to_string());
                    let to_uuid = workflow.nodes.get(to_node).map(|n| n.id.to_string());

                    if let (Some(from_uuid), Some(to_uuid)) = (from_uuid, to_uuid) {
                        if let Some(conn) = get_connection() {
                            let _ = conn.reducers.delete_edge_by_pins(
                                workflow_id,
                                from_uuid,
                                format!("output_{}", from_pin),
                                to_uuid,
                                format!("input_{}", to_pin),
                            );
                        }
                    }
                }
            }

            Message::SelectionChanged(selected) => {
                self.selected_nodes = selected.iter().copied().collect();

                // Send selection to SpacetimeDB for collaboration
                if let Some(workflow_id) = &self.current_workflow_id {
                    if let Some(conn) = get_connection() {
                        // Convert node indices to UUIDs
                        let uuids: Vec<String> = selected.iter()
                            .filter_map(|&id| self.current_workflow.as_ref()
                                .and_then(|w| w.nodes.get(id))
                                .map(|n| format!("\"{}\"", n.id)))
                            .collect();
                        let node_uuids_json = format!("[{}]", uuids.join(","));
                        let _ = conn.reducers.update_selection(workflow_id.clone(), node_uuids_json);
                    }
                }
            }

            Message::DeleteNodes(node_ids) => {
                // Collect node UUIDs for SpacetimeDB deletion before modifying local state
                let mut nodes_to_delete: Vec<(String, String)> = Vec::new();
                if let (Some(workflow_id), Some(workflow)) = (&self.current_workflow_id, &self.current_workflow) {
                    for &id in &node_ids {
                        if let Some(node) = workflow.nodes.get(id) {
                            nodes_to_delete.push((workflow_id.clone(), node.id.to_string()));
                        }
                    }
                }

                // Remove nodes locally (in reverse order to maintain indices)
                let mut sorted_ids: Vec<_> = node_ids.into_iter().collect();
                sorted_ids.sort_by(|a, b| b.cmp(a));

                for id in sorted_ids {
                    if id < self.node_positions.len() {
                        self.node_positions.remove(id);
                    }
                    if let Some(ref mut workflow) = self.current_workflow {
                        if id < workflow.nodes.len() {
                            workflow.nodes.remove(id);
                        }
                    }
                    // Remove edges connected to this node
                    self.edges
                        .retain(|(from, to)| from.node_id != id && to.node_id != id);
                }

                // Fire and forget - server will echo back via callbacks
                if let Some(conn) = get_connection() {
                    for (workflow_id, node_uuid) in nodes_to_delete {
                        let _ = conn.reducers.delete_node(workflow_id, node_uuid);
                    }
                }
            }

            // Command palette messages
            Message::ToggleCommandPalette => {
                self.command_palette_open = !self.command_palette_open;
                if self.command_palette_open {
                    self.palette_view = PaletteView::Main;
                    self.palette_selected_index = 0;
                    return focus_input();
                } else {
                    self.command_input.clear();
                    self.palette_view = PaletteView::Main;
                    self.palette_selected_index = 0;
                }
            }

            Message::CommandPaletteInput(input) => {
                self.command_input = input;
                self.palette_selected_index = 0;
            }

            Message::CommandPaletteNavigateUp => {
                if !self.command_palette_open {
                    return Task::none();
                }
                let (_, commands) = self.build_palette_commands();
                let filtered_count = get_filtered_count(&self.command_input, &commands);
                let new_index = navigate_up(self.palette_selected_index, filtered_count);
                return self.update(Message::CommandPaletteNavigate(new_index));
            }

            Message::CommandPaletteNavigateDown => {
                if !self.command_palette_open {
                    return Task::none();
                }
                let (_, commands) = self.build_palette_commands();
                let filtered_count = get_filtered_count(&self.command_input, &commands);
                let new_index = navigate_down(self.palette_selected_index, filtered_count);
                return self.update(Message::CommandPaletteNavigate(new_index));
            }

            Message::CommandPaletteNavigate(new_index) => {
                if !self.command_palette_open {
                    return Task::none();
                }
                self.palette_selected_index = new_index;
            }

            Message::CommandPaletteSelect(index) => {
                if !self.command_palette_open {
                    return Task::none();
                }
                self.palette_selected_index = index;
                return self.update(Message::CommandPaletteConfirm);
            }

            Message::CommandPaletteConfirm => {
                if !self.command_palette_open {
                    return Task::none();
                }
                let (_, commands) = self.build_palette_commands();
                let Some(original_idx) = get_filtered_command_index(
                    &self.command_input,
                    &commands,
                    self.palette_selected_index,
                ) else {
                    return Task::none();
                };

                use iced_palette::CommandAction;
                let cmd = &commands[original_idx];
                match &cmd.action {
                    CommandAction::Message(msg) => {
                        let msg = msg.clone();
                        self.command_input.clear();
                        self.palette_selected_index = 0;
                        return self.update(msg);
                    }
                    _ => {}
                }
            }

            Message::CommandPaletteCancel => {
                if !self.command_palette_open {
                    return Task::none();
                }
                self.command_palette_open = false;
                self.command_input.clear();
                self.palette_view = PaletteView::Main;
                self.palette_selected_index = 0;
            }

            Message::ExecuteShortcut(cmd_id) => {
                match cmd_id.as_str() {
                    "add_node" => {
                        self.command_palette_open = true;
                        self.palette_view = PaletteView::Submenu("nodes".to_string());
                        self.palette_selected_index = 0;
                        self.command_input.clear();
                        return focus_input();
                    }
                    "change_theme" => {
                        self.command_palette_open = true;
                        self.palette_view = PaletteView::Submenu("themes".to_string());
                        self.palette_selected_index = 0;
                        self.command_input.clear();
                        return focus_input();
                    }
                    _ => {}
                }
            }

            Message::NavigateToSubmenu(submenu) => {
                self.palette_view = PaletteView::Submenu(submenu);
                self.command_input.clear();
                self.palette_selected_index = 0;
                return focus_input();
            }

            Message::NavigateBack => {
                self.palette_view = PaletteView::Main;
                self.command_input.clear();
                self.palette_selected_index = 0;
                return focus_input();
            }

            Message::SpawnNode { node_type, name } => {
                // Spawn node at center of viewport
                let x = 400.0;
                let y = 300.0;

                if let Some(workflow_id) = &self.current_workflow_id {
                    // Use the SAME UUID for both local and SpacetimeDB
                    let node_id = Uuid::new_v4();
                    let node_uuid = node_id.to_string();

                    // Add locally with the same UUID
                    if let Some(ref mut workflow) = self.current_workflow {
                        let node = WorkflowNode::new(&node_type, (x, y))
                            .with_id(node_id)
                            .with_name(&name);
                        workflow.add_node(node);
                        self.node_positions.push(iced::Point::new(x, y));
                    }

                    // Sync to SpacetimeDB with the same UUID
                    if let Some(conn) = get_connection() {
                        let _ = conn.reducers.add_node(
                            workflow_id.clone(),
                            node_uuid,
                            node_type,
                            name,
                            x,
                            y,
                            "{}".to_string(),
                        );
                    }
                }

                self.command_palette_open = false;
                self.palette_view = PaletteView::Main;
            }

            Message::ChangeTheme(theme) => {
                self.current_theme = theme;
                self.command_palette_open = false;
                self.command_input.clear();
                self.palette_view = PaletteView::Main;
            }

            Message::NicknameInputChanged(input) => {
                self.nickname_input = input;
            }

            Message::NicknameConfirm => {
                if !self.nickname_input.is_empty() {
                    // Save the nickname
                    self.user_nickname = Some(self.nickname_input.clone());
                    self.nickname_dialog_open = false;

                    // If there's a pending workflow, open it now
                    if let Some(workflow_id) = self.pending_workflow_id.take() {
                        return self.update(Message::OpenWorkflowEditor(workflow_id));
                    }
                }
            }

            Message::NicknameCancel => {
                self.nickname_dialog_open = false;
                self.pending_workflow_id = None;
                self.nickname_input.clear();
            }

            Message::CursorMoved { x, y } => {
                // Send cursor position to SpacetimeDB (for other users to see)
                if self.current_workflow_id.is_some() {
                    if let Some(conn) = get_connection() {
                        let _ = conn.reducers.update_cursor(x, y);
                    }
                }
            }

            Message::DragStarted(drag_info) => {
                // Send drag start to SpacetimeDB for collaboration
                if let Some(workflow_id) = &self.current_workflow_id {
                    if let Some(conn) = get_connection() {
                        let (drag_type, node_uuids, from_pin, x, y) = match &drag_info {
                            DragInfo::Node { node_id } => {
                                let uuid = self.current_workflow.as_ref()
                                    .and_then(|w| w.nodes.get(*node_id))
                                    .map(|n| n.id.to_string())
                                    .unwrap_or_default();
                                ("node", format!("[\"{}\"]", uuid), String::new(), 0.0, 0.0)
                            }
                            DragInfo::Group { node_ids } => {
                                let uuids: Vec<String> = node_ids.iter()
                                    .filter_map(|id| self.current_workflow.as_ref()
                                        .and_then(|w| w.nodes.get(*id))
                                        .map(|n| format!("\"{}\"", n.id)))
                                    .collect();
                                ("group", format!("[{}]", uuids.join(",")), String::new(), 0.0, 0.0)
                            }
                            DragInfo::Edge { from_node, from_pin } => {
                                let uuid = self.current_workflow.as_ref()
                                    .and_then(|w| w.nodes.get(*from_node))
                                    .map(|n| n.id.to_string())
                                    .unwrap_or_default();
                                ("edge", "[]".to_string(), format!("{}:{}", uuid, from_pin), 0.0, 0.0)
                            }
                            DragInfo::BoxSelect { start_x, start_y } => {
                                ("box_select", "[]".to_string(), String::new(), *start_x, *start_y)
                            }
                            DragInfo::EdgeVertex { edge_index, vertex_index } => {
                                ("edge_vertex", "[]".to_string(), format!("{}:{}", edge_index, vertex_index), 0.0, 0.0)
                            }
                        };
                        let _ = conn.reducers.start_drag(
                            workflow_id.clone(),
                            drag_type.to_string(),
                            node_uuids,
                            from_pin,
                            x,
                            y,
                        );
                    }
                }
            }

            Message::DragUpdated { x, y } => {
                // Send drag update to SpacetimeDB
                if self.current_workflow_id.is_some() {
                    if let Some(conn) = get_connection() {
                        let _ = conn.reducers.update_drag(x, y);
                    }
                }
            }

            Message::DragEnded => {
                // Send drag end to SpacetimeDB
                if self.current_workflow_id.is_some() {
                    if let Some(conn) = get_connection() {
                        let _ = conn.reducers.end_drag();
                    }
                }
            }

            Message::Tick => {
                // Animation tick - just triggers redraw
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let content = match self.view {
            AppView::WorkflowList => self.view_workflow_list(),
            AppView::WorkflowEditor => self.view_workflow_editor(),
        };

        // Show nickname dialog overlay if open
        if self.nickname_dialog_open {
            stack!(content, self.view_nickname_dialog())
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            content
        }
    }

    fn view_nickname_dialog(&self) -> Element<'_, Message> {
        let color = Color::from_rgb(
            ((self.user_color >> 16) & 0xFF) as f32 / 255.0,
            ((self.user_color >> 8) & 0xFF) as f32 / 255.0,
            (self.user_color & 0xFF) as f32 / 255.0,
        );

        let color_indicator = container(text(" "))
            .width(24)
            .height(24)
            .style(move |_theme: &Theme| {
                container::Style {
                    background: Some(iced::Background::Color(color)),
                    border: iced::Border {
                        color: Color::WHITE,
                        width: 2.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            });

        let dialog_content = container(
            column![
                text("Enter your nickname").size(20),
                text("This will be shown to other collaborators").size(12),
                row![
                    color_indicator,
                    text_input("Your name...", &self.nickname_input)
                        .on_input(Message::NicknameInputChanged)
                        .on_submit(Message::NicknameConfirm)
                        .width(Length::Fill)
                        .padding(8),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                row![
                    button("Cancel").on_press(Message::NicknameCancel),
                    button("Join").on_press(Message::NicknameConfirm),
                ]
                .spacing(10),
            ]
            .spacing(15)
            .padding(20)
            .width(300),
        )
        .style(|theme: &Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(iced::Background::Color(palette.background.weak.color)),
                border: iced::Border {
                    color: palette.background.strong.color,
                    width: 1.0,
                    radius: 8.0.into(),
                },
                shadow: iced::Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 12.0,
                },
                ..Default::default()
            }
        });

        mouse_area(
            container(dialog_content)
                .center(Length::Fill)
                .style(|_theme: &Theme| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                    ..Default::default()
                }),
        )
        .on_press(Message::NicknameCancel)
        .into()
    }

    fn view_workflow_list(&self) -> Element<'_, Message> {
        let status_text = match &self.connection_status {
            ConnectionStatus::Disconnected => "Disconnected",
            ConnectionStatus::Connecting => "Connecting...",
            ConnectionStatus::Connected => "Connected",
            ConnectionStatus::Error(e) => e.as_str(),
        };

        let status_row = row![
            text(format!("SpacetimeDB: {}", status_text)).size(12),
        ]
        .padding(5);

        let title = text("ngwa - Workflows").size(28);

        let workflow_list = scrollable(
            column(
                self.workflows
                    .iter()
                    .map(|w| {
                        let id = w.id.clone();
                        let delete_id = w.id.clone();
                        row![
                            text(&w.name).width(Length::Fill),
                            button("Open").on_press(Message::OpenWorkflowEditor(id)),
                            button("Delete").on_press(Message::DeleteWorkflow(delete_id)),
                        ]
                        .spacing(10)
                        .padding(10)
                        .into()
                    })
                    .collect::<Vec<_>>(),
            )
            .spacing(5),
        )
        .height(Length::Fill);

        let new_workflow_row = row![
            text_input("New workflow name...", &self.new_workflow_name)
                .on_input(Message::NewWorkflowNameChanged)
                .on_submit(Message::CreateWorkflow)
                .width(Length::Fill),
            button("Create").on_press(Message::CreateWorkflow),
        ]
        .spacing(10);

        container(
            column![status_row, title, workflow_list, new_workflow_row]
                .spacing(20)
                .padding(20),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn view_workflow_editor(&self) -> Element<'_, Message> {
        let workflow_name = self
            .current_workflow
            .as_ref()
            .map(|w| w.name.as_str())
            .unwrap_or("Untitled");

        // Build user avatars for collaborators
        let user_avatars: Element<'_, Message> = if self.remote_users.is_empty() {
            text("").into()
        } else {
            let avatars: Vec<Element<'_, Message>> = self.remote_users.values()
                .map(|user| {
                    let color = Color::from_rgb(
                        ((user.color >> 16) & 0xFF) as f32 / 255.0,
                        ((user.color >> 8) & 0xFF) as f32 / 255.0,
                        (user.color & 0xFF) as f32 / 255.0,
                    );
                    // Get first letter of nickname for avatar
                    let initial = user.nickname.chars().next()
                        .map(|c| c.to_uppercase().to_string())
                        .unwrap_or_else(|| "?".to_string());

                    container(
                        text(initial).size(12)
                    )
                    .width(28)
                    .height(28)
                    .center_x(28)
                    .center_y(28)
                    .style(move |_theme: &Theme| {
                        container::Style {
                            background: Some(iced::Background::Color(color)),
                            border: iced::Border {
                                color: Color::WHITE,
                                width: 2.0,
                                radius: 14.0.into(),
                            },
                            ..Default::default()
                        }
                    })
                    .into()
                })
                .collect();

            row(avatars).spacing(4).into()
        };

        // Show our own nickname
        let my_name = self.user_nickname.as_deref().unwrap_or("Me");
        let my_color = Color::from_rgb(
            ((self.user_color >> 16) & 0xFF) as f32 / 255.0,
            ((self.user_color >> 8) & 0xFF) as f32 / 255.0,
            (self.user_color & 0xFF) as f32 / 255.0,
        );

        // User count indicator
        let user_count = self.remote_users.len();
        let user_count_text = if user_count > 0 {
            format!("{} online", user_count + 1) // +1 for ourselves
        } else {
            "1 online".to_string()
        };

        let header = row![
            button("< Back").on_press(Message::OpenWorkflowList),
            text(workflow_name).size(24),
            iced::widget::Space::new().width(Length::Fill),
            text(user_count_text).size(12),
            user_avatars,
            container(text(my_name).size(12))
                .padding([4, 8])
                .style(move |_theme: &Theme| {
                    container::Style {
                        background: Some(iced::Background::Color(my_color)),
                        border: iced::Border {
                            color: Color::WHITE,
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    }
                }),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center)
        .width(Length::Fill)
        .padding(10);

        // Build the node graph
        let mut ng: NodeGraph<Message, Theme, iced::Renderer> = node_graph()
            .on_connect(
                |from_node, from_pin, to_node, to_pin| Message::EdgeConnected {
                    from_node,
                    from_pin,
                    to_node,
                    to_pin,
                },
            )
            .on_disconnect(
                |from_node, from_pin, to_node, to_pin| Message::EdgeDisconnected {
                    from_node,
                    from_pin,
                    to_node,
                    to_pin,
                },
            )
            .on_move(|node_id, position| Message::NodeMoved { node_id, position })
            .on_group_move(|node_ids, delta| Message::GroupMoved { node_ids, delta })
            .on_select(Message::SelectionChanged)
            .on_delete(Message::DeleteNodes)
            // Drag callbacks for real-time collaboration
            .on_drag_start(Message::DragStarted)
            .on_drag_update(|x, y| Message::DragUpdated { x, y })
            .on_drag_end(|| Message::DragEnded);

        // Add nodes
        if let Some(workflow) = &self.current_workflow {
            for (i, node) in workflow.nodes.iter().enumerate() {
                let position = self
                    .node_positions
                    .get(i)
                    .copied()
                    .unwrap_or(iced::Point::new(node.position.0, node.position.1));

                let node_content = create_node_widget(node, &self.current_theme);
                ng.push_node(position, node_content);
            }
        }

        // Add edges
        for (from, to) in &self.edges {
            ng.push_edge(*from, *to);
        }

        let graph_view: Element<Message> = ng.into();

        let content = container(column![header, graph_view].spacing(0))
            .width(Length::Fill)
            .height(Length::Fill);

        // Show command palette overlay if open
        if self.command_palette_open {
            let (_, commands) = self.build_palette_commands();

            stack!(
                content,
                command_palette(
                    &self.command_input,
                    &commands,
                    self.palette_selected_index,
                    Message::CommandPaletteInput,
                    Message::CommandPaletteSelect,
                    || Message::CommandPaletteCancel
                )
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            content.into()
        }
    }

    fn theme(&self) -> Theme {
        self.current_theme.clone()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            iced::event::listen_with(handle_keyboard_event),
            window::frames().map(|_| Message::Tick),
        ])
    }

    /// Get main commands with shortcuts for subscription handling
    fn get_main_commands_with_shortcuts() -> Vec<Command<Message>> {
        vec![
            command("add_node", "Add Node")
                .description("Add a new node to the graph")
                .shortcut(Shortcut::cmd('n'))
                .action(Message::ExecuteShortcut("add_node".to_string())),
            command("change_theme", "Change Theme")
                .description("Switch to a different color theme")
                .shortcut(Shortcut::cmd('t'))
                .action(Message::ExecuteShortcut("change_theme".to_string())),
        ]
    }

    /// Build commands based on current palette view
    fn build_palette_commands(&self) -> (&'static str, Vec<Command<Message>>) {
        match &self.palette_view {
            PaletteView::Main => {
                let commands = vec![
                    command("add_node", "Add Node")
                        .description("Add a new node to the graph")
                        .shortcut(Shortcut::cmd('n'))
                        .action(Message::NavigateToSubmenu("nodes".to_string())),
                    command("change_theme", "Change Theme")
                        .description("Switch to a different color theme")
                        .shortcut(Shortcut::cmd('t'))
                        .action(Message::NavigateToSubmenu("themes".to_string())),
                ];
                ("Command Palette", commands)
            }
            PaletteView::Submenu(submenu) if submenu == "nodes" => {
                let commands = get_node_types()
                    .iter()
                    .map(|info| {
                        command(info.type_id, info.display_name)
                            .description(info.description)
                            .action(Message::SpawnNode {
                                node_type: info.type_id.to_string(),
                                name: info.display_name.to_string(),
                            })
                    })
                    .collect();
                ("Add Node", commands)
            }
            PaletteView::Submenu(submenu) if submenu == "themes" => {
                let commands = get_available_themes()
                    .iter()
                    .map(|(theme, name)| {
                        command(*name, *name).action(Message::ChangeTheme(theme.clone()))
                    })
                    .collect();
                ("Choose Theme", commands)
            }
            _ => ("Command Palette", vec![]),
        }
    }
}

/// Keyboard event handler for subscriptions
fn handle_keyboard_event(
    event: Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    match event {
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            // Ctrl+Space or Ctrl+K: Toggle palette
            if is_toggle_shortcut(&key, modifiers) {
                return Some(Message::ToggleCommandPalette);
            }

            // Also allow Ctrl+K
            if modifiers.command() {
                if let keyboard::Key::Character(c) = &key {
                    if c.as_str() == "k" {
                        return Some(Message::ToggleCommandPalette);
                    }
                }
            }

            // Global shortcuts with Ctrl/Cmd (Ctrl+N, Ctrl+T, etc.)
            if modifiers.command() {
                let main_commands = NgwaApp::get_main_commands_with_shortcuts();
                if let Some(cmd_id) = find_matching_shortcut(&main_commands, &key, modifiers) {
                    return Some(Message::ExecuteShortcut(cmd_id.to_string()));
                }
            }

            // Navigation keys
            match key {
                keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                    Some(Message::CommandPaletteNavigateUp)
                }
                keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                    Some(Message::CommandPaletteNavigateDown)
                }
                keyboard::Key::Named(keyboard::key::Named::Enter) => {
                    Some(Message::CommandPaletteConfirm)
                }
                keyboard::Key::Named(keyboard::key::Named::Escape) => {
                    Some(Message::CommandPaletteCancel)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Node type information for the command palette
struct NodeTypeInfo {
    type_id: &'static str,
    display_name: &'static str,
    description: &'static str,
    _category: &'static str,
}

/// Get all available node types
fn get_node_types() -> Vec<NodeTypeInfo> {
    vec![
        // Triggers
        NodeTypeInfo {
            type_id: "manual_trigger",
            display_name: "Manual Trigger",
            description: "Manually trigger workflow execution",
            _category: "Trigger",
        },
        NodeTypeInfo {
            type_id: "cron_trigger",
            display_name: "Cron Trigger",
            description: "Trigger workflow on a schedule",
            _category: "Trigger",
        },
        NodeTypeInfo {
            type_id: "webhook_trigger",
            display_name: "Webhook Trigger",
            description: "Trigger workflow via HTTP webhook",
            _category: "Trigger",
        },

        // Network
        NodeTypeInfo {
            type_id: "http_request",
            display_name: "HTTP Request",
            description: "Make HTTP requests to external APIs",
            _category: "Network",
        },
        NodeTypeInfo {
            type_id: "graphql",
            display_name: "GraphQL",
            description: "Execute GraphQL queries",
            _category: "Network",
        },

        // Data
        NodeTypeInfo {
            type_id: "set",
            display_name: "Set",
            description: "Set or modify data values",
            _category: "Data",
        },
        NodeTypeInfo {
            type_id: "merge",
            display_name: "Merge",
            description: "Merge multiple inputs into one",
            _category: "Data",
        },
        NodeTypeInfo {
            type_id: "split",
            display_name: "Split",
            description: "Split data into multiple outputs",
            _category: "Data",
        },
        NodeTypeInfo {
            type_id: "filter",
            display_name: "Filter",
            description: "Filter data based on conditions",
            _category: "Data",
        },
        NodeTypeInfo {
            type_id: "sort",
            display_name: "Sort",
            description: "Sort data by field",
            _category: "Data",
        },
        NodeTypeInfo {
            type_id: "limit",
            display_name: "Limit",
            description: "Limit number of items",
            _category: "Data",
        },

        // Flow Control
        NodeTypeInfo {
            type_id: "if",
            display_name: "If",
            description: "Conditional branching",
            _category: "Flow",
        },
        NodeTypeInfo {
            type_id: "switch",
            display_name: "Switch",
            description: "Multi-way branching",
            _category: "Flow",
        },
        NodeTypeInfo {
            type_id: "loop",
            display_name: "Loop",
            description: "Loop over items",
            _category: "Flow",
        },
        NodeTypeInfo {
            type_id: "wait",
            display_name: "Wait",
            description: "Pause execution for a duration",
            _category: "Flow",
        },
        NodeTypeInfo {
            type_id: "stop",
            display_name: "Stop",
            description: "Stop workflow execution",
            _category: "Flow",
        },

        // Code
        NodeTypeInfo {
            type_id: "code",
            display_name: "Code",
            description: "Execute custom JavaScript code",
            _category: "Code",
        },
        NodeTypeInfo {
            type_id: "function",
            display_name: "Function",
            description: "Define reusable function",
            _category: "Code",
        },

        // Text
        NodeTypeInfo {
            type_id: "text",
            display_name: "Text",
            description: "Text manipulation",
            _category: "Text",
        },
        NodeTypeInfo {
            type_id: "markdown",
            display_name: "Markdown",
            description: "Convert markdown to HTML",
            _category: "Text",
        },
        NodeTypeInfo {
            type_id: "html",
            display_name: "HTML",
            description: "Parse or generate HTML",
            _category: "Text",
        },
        NodeTypeInfo {
            type_id: "regex",
            display_name: "Regex",
            description: "Regular expression matching",
            _category: "Text",
        },

        // Files
        NodeTypeInfo {
            type_id: "read_file",
            display_name: "Read File",
            description: "Read file from disk",
            _category: "Files",
        },
        NodeTypeInfo {
            type_id: "write_file",
            display_name: "Write File",
            description: "Write file to disk",
            _category: "Files",
        },

        // Database
        NodeTypeInfo {
            type_id: "postgres",
            display_name: "PostgreSQL",
            description: "Query PostgreSQL database",
            _category: "Database",
        },
        NodeTypeInfo {
            type_id: "mysql",
            display_name: "MySQL",
            description: "Query MySQL database",
            _category: "Database",
        },
        NodeTypeInfo {
            type_id: "mongodb",
            display_name: "MongoDB",
            description: "Query MongoDB database",
            _category: "Database",
        },
        NodeTypeInfo {
            type_id: "redis",
            display_name: "Redis",
            description: "Redis key-value operations",
            _category: "Database",
        },

        // AI
        NodeTypeInfo {
            type_id: "openai",
            display_name: "OpenAI",
            description: "OpenAI API (GPT, DALL-E)",
            _category: "AI",
        },
        NodeTypeInfo {
            type_id: "anthropic",
            display_name: "Anthropic",
            description: "Anthropic Claude API",
            _category: "AI",
        },

        // Messaging
        NodeTypeInfo {
            type_id: "email_send",
            display_name: "Send Email",
            description: "Send email via SMTP",
            _category: "Messaging",
        },
        NodeTypeInfo {
            type_id: "slack",
            display_name: "Slack",
            description: "Send Slack messages",
            _category: "Messaging",
        },
        NodeTypeInfo {
            type_id: "discord",
            display_name: "Discord",
            description: "Send Discord messages",
            _category: "Messaging",
        },

        // Utilities
        NodeTypeInfo {
            type_id: "debug",
            display_name: "Debug",
            description: "Log data for debugging",
            _category: "Utilities",
        },
        NodeTypeInfo {
            type_id: "comment",
            display_name: "Comment",
            description: "Add a comment to the workflow",
            _category: "Utilities",
        },
    ]
}

/// Get available themes
fn get_available_themes() -> Vec<(Theme, &'static str)> {
    vec![
        (Theme::Dark, "Dark"),
        (Theme::Light, "Light"),
        (Theme::Dracula, "Dracula"),
        (Theme::Nord, "Nord"),
        (Theme::SolarizedLight, "Solarized Light"),
        (Theme::SolarizedDark, "Solarized Dark"),
        (Theme::GruvboxLight, "Gruvbox Light"),
        (Theme::GruvboxDark, "Gruvbox Dark"),
        (Theme::CatppuccinLatte, "Catppuccin Latte"),
        (Theme::CatppuccinFrappe, "Catppuccin Frappe"),
        (Theme::CatppuccinMacchiato, "Catppuccin Macchiato"),
        (Theme::CatppuccinMocha, "Catppuccin Mocha"),
        (Theme::TokyoNight, "Tokyo Night"),
        (Theme::TokyoNightStorm, "Tokyo Night Storm"),
        (Theme::TokyoNightLight, "Tokyo Night Light"),
        (Theme::KanagawaWave, "Kanagawa Wave"),
        (Theme::KanagawaDragon, "Kanagawa Dragon"),
        (Theme::KanagawaLotus, "Kanagawa Lotus"),
        (Theme::Moonfly, "Moonfly"),
        (Theme::Nightfly, "Nightfly"),
        (Theme::Oxocarbon, "Oxocarbon"),
        (Theme::Ferra, "Ferra"),
    ]
}

/// Pin definition: (name, direction, color)
type PinDef = (&'static str, PinDirection, Color);

/// Get pin definitions for a node type: (inputs, outputs)
fn get_node_pins(node_type: &str) -> (Vec<PinDef>, Vec<PinDef>) {
    let trigger_color = Color::from_rgb(0.3, 0.9, 0.5);    // Green for trigger/flow
    let data_color = Color::from_rgb(0.3, 0.7, 0.9);       // Blue for data
    let string_color = Color::from_rgb(0.9, 0.7, 0.3);     // Orange for strings
    let bool_color = Color::from_rgb(0.9, 0.4, 0.4);       // Red for booleans
    let json_color = Color::from_rgb(0.7, 0.3, 0.9);       // Purple for JSON

    match node_type {
        // Triggers - only outputs
        "manual_trigger" | "cron_trigger" | "webhook_trigger" | "email_trigger"
        | "file_trigger" | "database_trigger" | "mqtt_trigger" | "kafka_trigger" => {
            (vec![], vec![("trigger", PinDirection::Output, trigger_color)])
        }

        // HTTP/API nodes
        "http_request" | "graphql_request" | "soap_request" | "grpc_call" => {
            (
                vec![("trigger", PinDirection::Input, trigger_color)],
                vec![
                    ("response", PinDirection::Output, json_color),
                    ("error", PinDirection::Output, bool_color),
                ],
            )
        }

        // Data transformation
        "set" | "rename_keys" | "remove_fields" | "xml_to_json" | "json_to_xml"
        | "csv_parse" | "csv_generate" | "base64_encode" | "base64_decode"
        | "compress" | "decompress" | "encrypt" | "decrypt" | "html_extract"
        | "markdown_to_html" | "date_time" | "aggregate" => {
            (
                vec![("input", PinDirection::Input, data_color)],
                vec![("output", PinDirection::Output, data_color)],
            )
        }

        // Flow control with multiple outputs
        "if" | "filter" => {
            (
                vec![("input", PinDirection::Input, data_color)],
                vec![
                    ("true", PinDirection::Output, trigger_color),
                    ("false", PinDirection::Output, bool_color),
                ],
            )
        }
        "switch" | "route" => {
            (
                vec![("input", PinDirection::Input, data_color)],
                vec![
                    ("case1", PinDirection::Output, trigger_color),
                    ("case2", PinDirection::Output, trigger_color),
                    ("default", PinDirection::Output, data_color),
                ],
            )
        }

        // Code execution
        "code" | "function" | "expression" => {
            (
                vec![("input", PinDirection::Input, data_color)],
                vec![
                    ("output", PinDirection::Output, data_color),
                    ("error", PinDirection::Output, bool_color),
                ],
            )
        }

        // Merge/Split
        "merge" | "compare" => {
            (
                vec![
                    ("input1", PinDirection::Input, data_color),
                    ("input2", PinDirection::Input, data_color),
                ],
                vec![("output", PinDirection::Output, data_color)],
            )
        }
        "split_in_batches" | "loop" | "split_out" => {
            (
                vec![("input", PinDirection::Input, data_color)],
                vec![
                    ("item", PinDirection::Output, data_color),
                    ("done", PinDirection::Output, trigger_color),
                ],
            )
        }

        // Database operations
        "postgres" | "mysql" | "mongodb" | "redis" | "elasticsearch" | "sqlite"
        | "supabase" | "airtable" | "notion_database" => {
            (
                vec![
                    ("trigger", PinDirection::Input, trigger_color),
                    ("query", PinDirection::Input, string_color),
                ],
                vec![
                    ("result", PinDirection::Output, json_color),
                    ("error", PinDirection::Output, bool_color),
                ],
            )
        }

        // File operations
        "read_file" | "write_file" | "ftp" | "sftp" | "s3" | "google_drive"
        | "dropbox" | "onedrive" => {
            (
                vec![("trigger", PinDirection::Input, trigger_color)],
                vec![
                    ("data", PinDirection::Output, data_color),
                    ("error", PinDirection::Output, bool_color),
                ],
            )
        }

        // Communication
        "send_email" | "slack" | "discord" | "telegram" | "twilio" | "push_notification" => {
            (
                vec![
                    ("trigger", PinDirection::Input, trigger_color),
                    ("message", PinDirection::Input, string_color),
                ],
                vec![
                    ("success", PinDirection::Output, trigger_color),
                    ("error", PinDirection::Output, bool_color),
                ],
            )
        }

        // AI/ML
        "openai" | "anthropic" | "huggingface" | "text_classifier" | "sentiment_analysis" => {
            (
                vec![
                    ("trigger", PinDirection::Input, trigger_color),
                    ("prompt", PinDirection::Input, string_color),
                ],
                vec![
                    ("response", PinDirection::Output, string_color),
                    ("error", PinDirection::Output, bool_color),
                ],
            )
        }

        // Utility
        "wait" | "delay" => {
            (
                vec![("trigger", PinDirection::Input, trigger_color)],
                vec![("done", PinDirection::Output, trigger_color)],
            )
        }
        "no_op" | "sticky_note" => (vec![], vec![]),
        "error_trigger" => {
            (
                vec![],
                vec![
                    ("error", PinDirection::Output, bool_color),
                    ("workflow", PinDirection::Output, string_color),
                ],
            )
        }
        "respond_to_webhook" | "execute_workflow" => {
            (
                vec![
                    ("trigger", PinDirection::Input, trigger_color),
                    ("data", PinDirection::Input, data_color),
                ],
                vec![("done", PinDirection::Output, trigger_color)],
            )
        }

        // Default: single input/output
        _ => {
            (
                vec![("input", PinDirection::Input, data_color)],
                vec![("output", PinDirection::Output, data_color)],
            )
        }
    }
}

/// Create a widget for displaying a node with pins
fn create_node_widget<'a>(node: &WorkflowNode, theme: &'a Theme) -> Element<'a, Message> {
    let (inputs, outputs) = get_node_pins(&node.node_type);
    let style = NodeContentStyle::process(theme);

    // Build pin list - each pin in a column
    let mut pin_elements: Vec<Element<'a, Message>> = Vec::new();

    // Add input pins (left side)
    for (name, dir, color) in &inputs {
        pin_elements.push(
            node_pin(
                PinSide::Left,
                container(text(*name).size(10)).padding([0, 8]),
            )
            .direction(*dir)
            .pin_type(*name)
            .color(*color)
            .into(),
        );
    }

    // Add output pins (right side)
    for (name, dir, color) in &outputs {
        pin_elements.push(
            node_pin(
                PinSide::Right,
                container(text(*name).size(10)).padding([0, 8]),
            )
            .direction(*dir)
            .pin_type(*name)
            .color(*color)
            .into(),
        );
    }

    let pin_section = container(
        iced::widget::Column::with_children(pin_elements).spacing(2),
    )
    .padding([6, 0]);

    column![node_title_bar(&node.name, style), pin_section]
        .width(160.0)
        .into()
}
