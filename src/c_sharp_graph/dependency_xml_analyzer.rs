use std::collections::HashMap;
use std::collections::HashSet;
use std::iter::DoubleEndedIterator;
use std::iter::Extend;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::Reader;
use stack_graphs::arena::Handle;
use stack_graphs::graph;
use stack_graphs::graph::File;
use stack_graphs::graph::Node;
use stack_graphs::graph::StackGraph;
use tracing::error;
use tracing::info;
use tree_sitter_stack_graphs::BuildError;
use tree_sitter_stack_graphs::CancellationFlag;
use tree_sitter_stack_graphs::FileAnalyzer;

const MEMBER_NAME: QName = QName(b"member");

pub struct DepXMLFileAnalyzer {}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NodeInfo {
    symbol: String,
    syntax_type: String,
}

pub struct EdgeInfo {
    source: NodeInfo,
    sink: NodeInfo,
}

impl FileAnalyzer for DepXMLFileAnalyzer {
    fn build_stack_graph_into<'a>(
        &self,
        stack_graph: &mut StackGraph,
        file: Handle<File>,
        _path: &Path,
        source: &str,
        _all_paths: &mut dyn Iterator<Item = &'a Path>,
        globals: &HashMap<String, String>,
        _cancellation_flag: &dyn CancellationFlag,
    ) -> Result<(), tree_sitter_stack_graphs::BuildError> {
        let mut reader = Reader::from_str(source);

        reader.config_mut().trim_text(true);
        info!("globals {:#?}", globals);

        let mut inter_node_info: HashSet<NodeInfo> = HashSet::new();
        let mut inter_edge_info: Vec<EdgeInfo> = vec![];
        loop {
            match reader.read_event() {
                Err(e) => {
                    error!("got errror {}", e);
                    return Err(BuildError::ParseError);
                }
                Ok(Event::Eof) => {
                    break;
                }
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    if e.name() == MEMBER_NAME {
                        let member_name = e.attributes().find(|e| match e {
                            Ok(e) => e.value.len() > 0,
                            Err(_) => false,
                        });
                        if member_name.is_none() {
                            continue;
                        }
                        let member_name = member_name.unwrap().unwrap();
                        let member_name = String::from_utf8_lossy(&member_name.value).to_string();
                        let parts: Vec<&str> = member_name.split(":").collect();
                        if parts.len() != 2 {
                            info!("unable to get correct parts: {}", &member_name);
                            continue;
                        }
                        let (nodes, mut edges) =
                            self.handle_member(parts.first().unwrap(), parts.last().unwrap());
                        inter_node_info.extend(nodes.iter().cloned());
                        inter_edge_info.append(&mut edges);
                    }
                    continue;
                }
                _ => (),
            }
        }
        info!(
            "got {} nodes and {} edges to be created",
            &inter_node_info.len(),
            &inter_edge_info.len()
        );

        let mut symbol_to_graph_node: HashMap<String, Handle<Node>> = HashMap::new();
        for node in inter_node_info {
            let id = stack_graph.new_node_id(file);
            let symbol = stack_graph.add_symbol(&node.symbol);
            let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
            if node_handle.is_none() {
                info!("node_handle is none???");
                continue;
            }
            let node_handle = node_handle.unwrap();
            let syntax_type = stack_graph.add_string(&node.syntax_type);
            let source_info = stack_graph.source_info_mut(node_handle);
            source_info.syntax_type = syntax_type.into();
            symbol_to_graph_node.insert(node.symbol.clone(), node_handle);
        }

        let mut edge_tracking: HashMap<Handle<Node>, HashMap<Handle<Node>, bool>> = HashMap::new();
        let mut edge_tracking_number: usize = 0;
        for edge in inter_edge_info {
            let source_node_handle = symbol_to_graph_node.get(&edge.source.symbol);
            let sink_node_handle = symbol_to_graph_node.get(&edge.sink.symbol);

            if source_node_handle.is_none() || sink_node_handle.is_none() {
                info!("unable to get symbols for edge");
                continue;
            }
            let source_node_handle = source_node_handle.unwrap();
            let sink_node_handle = sink_node_handle.unwrap();
            if let Some(edge_map) = edge_tracking.get(source_node_handle) {
                if edge_map.get(sink_node_handle).is_some() {
                    continue;
                }
            }
            stack_graph.add_edge(*source_node_handle, *sink_node_handle, 0);
            edge_tracking_number += 1;
            if let Some(edge_map) = edge_tracking.get_mut(source_node_handle) {
                edge_map.insert(*sink_node_handle, true);
            } else {
                let mut new_map: HashMap<Handle<Node>, bool> = HashMap::new();
                new_map.insert(*sink_node_handle, true);
                edge_tracking.insert(*source_node_handle, new_map);
            }
        }

        info!(
            "created {} graph nodes {} edge nodes",
            symbol_to_graph_node.len(),
            &edge_tracking_number
        );
        Ok(())
    }
}

impl DepXMLFileAnalyzer {
    fn handle_member(&self, member_type: &str, name: &str) -> (Vec<NodeInfo>, Vec<EdgeInfo>) {
        match member_type {
            // namespace.
            "N" => {
                let node = NodeInfo {
                    symbol: name.to_string(),
                    syntax_type: "namespace_defin".to_string(),
                };
                (vec![node], vec![])
            }
            // type, field and property
            "T" => {
                let mut parts = name.split('.');
                let mut nodes: Vec<NodeInfo> = vec![];
                let mut edges: Vec<EdgeInfo> = vec![];
                let part = parts.next_back();
                if part.is_none() {
                    return (vec![], vec![]);
                }
                let type_name = NodeInfo {
                    symbol: part.unwrap().to_string(),
                    syntax_type: "class_def".to_string(),
                };
                nodes.push(type_name.clone());
                let mut prev_node = type_name;
                while let Some(part) = parts.next_back() {
                    let node = NodeInfo {
                        symbol: part.to_string(),
                        syntax_type: "namespace_declaration".to_string(),
                    };
                    nodes.push(node.clone());
                    edges.push(EdgeInfo {
                        source: node.clone(),
                        sink: prev_node.clone(),
                    });
                    prev_node = node.clone();
                }
                (nodes, edges)
            }
            "F" | "P" => {
                let mut parts = name.split('.');
                let mut nodes: Vec<NodeInfo> = vec![];
                let mut edges: Vec<EdgeInfo> = vec![];
                let part = parts.next_back();
                if part.is_none() {
                    return (vec![], vec![]);
                }
                let type_name = NodeInfo {
                    symbol: part.unwrap().to_string(),
                    syntax_type: "field_name".to_string(),
                };
                nodes.push(type_name.clone());
                let mut prev_node = type_name;
                while let Some(part) = parts.next_back() {
                    let node = NodeInfo {
                        symbol: part.to_string(),
                        syntax_type: "namespace_declaration".to_string(),
                    };
                    nodes.push(node.clone());
                    edges.push(EdgeInfo {
                        source: node.clone(),
                        sink: prev_node.clone(),
                    });
                    prev_node = node.clone();
                }
                (nodes, edges)
            }
            "M" => {
                let mut parts = name.split('.');
                let mut nodes: Vec<NodeInfo> = vec![];
                let mut edges: Vec<EdgeInfo> = vec![];
                let part = parts.next_back();
                if part.is_none() {
                    return (vec![], vec![]);
                }
                // Handle the name of the method here.
                // if #ctor means constructor.
                // for now we can ignore the parameters.
                let part = part.unwrap();
                let mut previous_node = if part.contains("#ctor") {
                    // Get the next back Symbol and that will be the symbol.
                    let part = parts.next_back();
                    if part.is_none() {
                        return (vec![], vec![]);
                    }
                    let node = NodeInfo {
                        symbol: part.unwrap().to_string(),
                        syntax_type: "method_name".to_string(),
                    };
                    nodes.push(node.clone());
                    node
                } else {
                    let node = NodeInfo {
                        symbol: part.to_string(),
                        syntax_type: "method_name".to_string(),
                    };
                    nodes.push(node.clone());
                    node
                };
                while let Some(part) = parts.next_back() {
                    let node = NodeInfo {
                        symbol: part.to_string(),
                        syntax_type: "namespace_declaration".to_string(),
                    };
                    nodes.push(node.clone());
                    edges.push(EdgeInfo {
                        source: node.clone(),
                        sink: previous_node.clone(),
                    });
                    previous_node = node.clone();
                }
                (nodes, edges)
            }
            _ => {
                info!("unable to handle: {} -- {}", member_type, name);
                (vec![], vec![])
            }
        }
    }
}
