//! ngwa-app: Desktop application for the ngwa workflow automation system
//!
//! This is the main entry point for the ngwa desktop application.
//! It provides a visual editor for creating and managing workflows.

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Element, Length, Task, Theme};
use iced_nodegraph::{node_graph, NodeGraph, PinReference};
use ngwa_core::{Workflow, WorkflowNode};
use std::collections::HashSet;
use uuid::Uuid;

fn main() -> iced::Result {
    iced::application(NgwaApp::new, NgwaApp::update, NgwaApp::view)
        .theme(NgwaApp::theme)
        .title("ngwa - Workflow Automation")
        .run()
}

/// Main application state
struct NgwaApp {
    /// Current view
    view: AppView,

    /// List of workflows
    workflows: Vec<WorkflowSummary>,

    /// Currently open workflow
    current_workflow: Option<Workflow>,

    /// Node positions (visual only)
    node_positions: Vec<iced::Point>,

    /// Edges in the graph
    edges: Vec<(PinReference, PinReference)>,

    /// Selected nodes
    selected_nodes: HashSet<usize>,

    /// New workflow name input
    new_workflow_name: String,

    /// Command palette state
    command_palette_open: bool,
    command_input: String,
}

/// Summary of a workflow for the list view
#[derive(Debug, Clone)]
struct WorkflowSummary {
    id: Uuid,
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
    // Navigation
    OpenWorkflowList,
    OpenWorkflowEditor(Uuid),

    // Workflow management
    CreateWorkflow,
    DeleteWorkflow(Uuid),
    NewWorkflowNameChanged(String),

    // Graph editing
    NodeMoved { node_id: usize, position: iced::Point },
    EdgeConnected { from_node: usize, from_pin: usize, to_node: usize, to_pin: usize },
    EdgeDisconnected { from_node: usize, from_pin: usize, to_node: usize, to_pin: usize },
    SelectionChanged(Vec<usize>),
    DeleteNodes(Vec<usize>),

    // Command palette
    ToggleCommandPalette,
    CommandInputChanged(String),
    ExecuteCommand(String),

    // Other
    Noop,
}

impl NgwaApp {
    fn new() -> Self {
        // Create a demo workflow
        let demo_workflow = Workflow::new("Demo Workflow");
        let demo_id = demo_workflow.id;

        Self {
            view: AppView::WorkflowList,
            workflows: vec![
                WorkflowSummary {
                    id: demo_id,
                    name: "Demo Workflow".to_string(),
                },
            ],
            current_workflow: None,
            node_positions: Vec::new(),
            edges: Vec::new(),
            selected_nodes: HashSet::new(),
            new_workflow_name: String::new(),
            command_palette_open: false,
            command_input: String::new(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenWorkflowList => {
                self.view = AppView::WorkflowList;
                self.current_workflow = None;
            }

            Message::OpenWorkflowEditor(id) => {
                // Create a demo workflow with some nodes
                let mut workflow = Workflow::new(
                    self.workflows
                        .iter()
                        .find(|w| w.id == id)
                        .map(|w| w.name.clone())
                        .unwrap_or_else(|| "Untitled".to_string()),
                );

                // Add some demo nodes
                let trigger = WorkflowNode::new("manual_trigger", (100.0, 150.0))
                    .with_name("Start");
                let http = WorkflowNode::new("http_request", (350.0, 150.0))
                    .with_name("Fetch Data");
                let set = WorkflowNode::new("set", (600.0, 150.0))
                    .with_name("Transform");

                workflow.add_node(trigger);
                workflow.add_node(http);
                workflow.add_node(set);

                self.node_positions = vec![
                    iced::Point::new(100.0, 150.0),
                    iced::Point::new(350.0, 150.0),
                    iced::Point::new(600.0, 150.0),
                ];

                self.edges = vec![
                    (PinReference::new(0, 0), PinReference::new(1, 0)),
                    (PinReference::new(1, 0), PinReference::new(2, 0)),
                ];

                self.current_workflow = Some(workflow);
                self.view = AppView::WorkflowEditor;
            }

            Message::CreateWorkflow => {
                if !self.new_workflow_name.is_empty() {
                    let workflow = Workflow::new(&self.new_workflow_name);
                    self.workflows.push(WorkflowSummary {
                        id: workflow.id,
                        name: workflow.name.clone(),
                    });
                    self.new_workflow_name.clear();
                }
            }

            Message::DeleteWorkflow(id) => {
                self.workflows.retain(|w| w.id != id);
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
            }

            Message::EdgeConnected { from_node, from_pin, to_node, to_pin } => {
                self.edges.push((
                    PinReference::new(from_node, from_pin),
                    PinReference::new(to_node, to_pin),
                ));
            }

            Message::EdgeDisconnected { from_node, from_pin, to_node, to_pin } => {
                self.edges.retain(|(from, to)| {
                    !(from.node_id == from_node
                        && from.pin_id == from_pin
                        && to.node_id == to_node
                        && to.pin_id == to_pin)
                });
            }

            Message::SelectionChanged(selected) => {
                self.selected_nodes = selected.into_iter().collect();
            }

            Message::DeleteNodes(node_ids) => {
                // Remove nodes (in reverse order to maintain indices)
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
                    self.edges.retain(|(from, to)| {
                        from.node_id != id && to.node_id != id
                    });
                }
            }

            Message::ToggleCommandPalette => {
                self.command_palette_open = !self.command_palette_open;
                if !self.command_palette_open {
                    self.command_input.clear();
                }
            }

            Message::CommandInputChanged(input) => {
                self.command_input = input;
            }

            Message::ExecuteCommand(_cmd) => {
                self.command_palette_open = false;
                self.command_input.clear();
            }

            Message::Noop => {}
        }

        Task::none()
    }

    fn view(&self) -> Element<Message> {
        match self.view {
            AppView::WorkflowList => self.view_workflow_list(),
            AppView::WorkflowEditor => self.view_workflow_editor(),
        }
    }

    fn view_workflow_list(&self) -> Element<Message> {
        let title = text("ngwa - Workflows")
            .size(28);

        let workflow_list = scrollable(
            column(
                self.workflows
                    .iter()
                    .map(|w| {
                        let id = w.id;
                        row![
                            text(&w.name).width(Length::Fill),
                            button("Open")
                                .on_press(Message::OpenWorkflowEditor(id)),
                            button("Delete")
                                .on_press(Message::DeleteWorkflow(id)),
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
            button("Create")
                .on_press(Message::CreateWorkflow),
        ]
        .spacing(10);

        container(
            column![title, workflow_list, new_workflow_row]
                .spacing(20)
                .padding(20),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn view_workflow_editor(&self) -> Element<Message> {
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
            .on_connect(|from_node, from_pin, to_node, to_pin| Message::EdgeConnected {
                from_node,
                from_pin,
                to_node,
                to_pin,
            })
            .on_disconnect(|from_node, from_pin, to_node, to_pin| Message::EdgeDisconnected {
                from_node,
                from_pin,
                to_node,
                to_pin,
            })
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

        container(
            column![header, graph_view]
                .spacing(0),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }
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
