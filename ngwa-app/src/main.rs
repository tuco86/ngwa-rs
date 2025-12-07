//! ngwa-app: Desktop application for the ngwa workflow automation system
//!
//! This is the main entry point for the ngwa desktop application.
//! It provides a visual editor for creating and managing workflows.

mod spacetime_sync;
mod spacetimedb_client;

use iced::widget::{button, column, container, row, scrollable, stack, text, text_input};
use iced::{keyboard, window, Element, Event, Length, Subscription, Task, Theme};
use iced_nodegraph::{node_graph, NodeGraph, PinReference};
use iced_palette::{
    Command, Shortcut, command, command_palette, find_matching_shortcut, focus_input,
    get_filtered_command_index, get_filtered_count, is_toggle_shortcut, navigate_down, navigate_up,
};
use ngwa_core::{Workflow, WorkflowNode};
use spacetimedb_client::{
    DbConnection,
    // Table access traits (needed for .workflow(), .workflow_node(), .workflow_edge())
    WorkflowTableAccess,
    WorkflowNodeTableAccess,
    WorkflowEdgeTableAccess,
    // Reducer traits (needed for calling reducers on conn.reducers)
    create_workflow_reducer::create_workflow,
    delete_workflow_reducer::delete_workflow,
    move_node_reducer::move_node,
    add_node_reducer::add_node,
    add_edge_reducer::add_edge,
    delete_edge_by_pins_reducer::delete_edge_by_pins,
    delete_node_reducer::delete_node,
};
use spacetimedb_sdk::{DbContext, Table, TableWithPrimaryKey};
use std::sync::OnceLock;
use std::collections::HashSet;
use uuid::Uuid;

const SPACETIMEDB_URI: &str = "http://localhost:3000";
const DATABASE_NAME: &str = "ngwa";

/// Global connection holder (initialized once)
static DB_CONNECTION: OnceLock<DbConnection> = OnceLock::new();

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
        };

        // Connect to SpacetimeDB after short delay
        (app, Task::perform(
            async {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            },
            |_| Message::Connect,
        ))
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

                                // Start the connection processing thread AFTER registering callbacks
                                conn.run_threaded();

                                // Subscribe to all tables
                                conn.subscription_builder()
                                    .subscribe([
                                        "SELECT * FROM workflow",
                                        "SELECT * FROM workflow_node",
                                        "SELECT * FROM workflow_edge",
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
                                if let Some(ref workflow) = self.current_workflow {
                                    // Find node index by UUID
                                    if let Some(idx) = workflow.nodes.iter()
                                        .position(|n| n.id.to_string() == node_uuid)
                                    {
                                        if idx < self.node_positions.len() {
                                            self.node_positions[idx] = iced::Point::new(x, y);
                                        }
                                    }
                                }
                            }
                        }
                        // Node and edge inserts/deletes could be handled here for live collaboration
                        // For now, we reload on view change
                        _ => {}
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
                self.view = AppView::WorkflowList;
                self.current_workflow = None;
                self.current_workflow_id = None;
                // No need to sync - callbacks keep the list up to date
            }

            Message::OpenWorkflowEditor(id) => {
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
                            let node = WorkflowNode::new(&node_data.node_type, (node_data.position_x, node_data.position_y))
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
                if let (Some(workflow_id), Some(ref workflow)) = (self.current_workflow_id.clone(), &self.current_workflow) {
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
                if let (Some(workflow_id), Some(ref workflow)) = (self.current_workflow_id.clone(), &self.current_workflow) {
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
                if let (Some(workflow_id), Some(ref workflow)) = (self.current_workflow_id.clone(), &self.current_workflow) {
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
                self.selected_nodes = selected.into_iter().collect();
            }

            Message::DeleteNodes(node_ids) => {
                // Collect node UUIDs for SpacetimeDB deletion before modifying local state
                let mut nodes_to_delete: Vec<(String, String)> = Vec::new();
                if let (Some(workflow_id), Some(ref workflow)) = (&self.current_workflow_id, &self.current_workflow) {
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
                    let node_uuid = Uuid::new_v4().to_string();

                    // Add locally
                    if let Some(ref mut workflow) = self.current_workflow {
                        let node = WorkflowNode::new(&node_type, (x, y)).with_name(&name);
                        workflow.add_node(node);
                        self.node_positions.push(iced::Point::new(x, y));
                    }

                    // Sync to SpacetimeDB
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

            Message::Tick => {
                // Animation tick - just triggers redraw
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        match self.view {
            AppView::WorkflowList => self.view_workflow_list(),
            AppView::WorkflowEditor => self.view_workflow_editor(),
        }
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

        let header = row![
            button("< Back").on_press(Message::OpenWorkflowList),
            text(workflow_name).size(24),
        ]
        .spacing(20)
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
            .on_select(Message::SelectionChanged)
            .on_delete(Message::DeleteNodes);

        // Add nodes
        if let Some(ref workflow) = self.current_workflow {
            for (i, node) in workflow.nodes.iter().enumerate() {
                let position = self
                    .node_positions
                    .get(i)
                    .copied()
                    .unwrap_or(iced::Point::new(node.position.0, node.position.1));

                let node_content = create_node_widget(node);
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

/// Create a widget for displaying a node
fn create_node_widget(node: &WorkflowNode) -> Element<'_, Message> {
    let node_type_display = match node.node_type.as_str() {
        "manual_trigger" => "Manual Trigger",
        "http_request" => "HTTP Request",
        "set" => "Set",
        "if" => "If",
        _ => node.node_type.as_str(),
    };

    container(
        column![
            text(node.name.clone()).size(14),
            text(node_type_display).size(11),
        ]
        .spacing(4),
    )
    .padding(8)
    .width(Length::Fixed(140.0))
    .into()
}
