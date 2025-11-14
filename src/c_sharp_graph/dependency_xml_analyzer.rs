use std::collections::HashMap;
use std::collections::HashSet;
use std::iter::DoubleEndedIterator;
use std::iter::Extend;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::Reader;
use stack_graphs::arena::Handle;
use stack_graphs::graph::File;
use stack_graphs::graph::Node;
use stack_graphs::graph::StackGraph;
use tracing::debug;
use tracing::error;
use tracing::info;
use tree_sitter_stack_graphs::BuildError;
use tree_sitter_stack_graphs::CancellationFlag;
use tree_sitter_stack_graphs::FileAnalyzer;

use crate::c_sharp_graph::query::SyntaxType;

const MEMBER_NAME: QName = QName(b"member");

pub struct DepXMLFileAnalyzer {}

#[derive(Clone, Debug)]
pub struct NodeInfo {
    symbol: String,
    syntax_type: SyntaxType,
}

#[derive(Debug)]
pub struct EdgeInfo {
    source: NodeInfo,
    sink: NodeInfo,
    precedence: i32,
}

impl FileAnalyzer for DepXMLFileAnalyzer {
    fn build_stack_graph_into<'a>(
        &self,
        stack_graph: &mut StackGraph,
        file: Handle<File>,
        path: &Path,
        source: &str,
        _all_paths: &mut dyn Iterator<Item = &'a Path>,
        globals: &HashMap<String, String>,
        _cancellation_flag: &dyn CancellationFlag,
    ) -> Result<(), tree_sitter_stack_graphs::BuildError> {
        let mut reader = Reader::from_str(source);

        reader.config_mut().trim_text(true);
        info!("globals {:#?}", globals);

        let mut inter_node_info: Vec<NodeInfo> = vec![];
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

        // Create Compilation Unit.
        let id = stack_graph.new_node_id(file);
        let symbol = stack_graph.add_symbol(path.to_string_lossy().as_ref());
        let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
        if node_handle.is_none() {
            error!("node_handle is none???");
            return Err(BuildError::UnknownSymbolType(
                "unable to handle comp unit".to_string(),
            ));
        }
        let node_handle = node_handle.unwrap();
        let syntax_type = stack_graph.add_string(SyntaxType::CompUnit.to_string());
        let source_info = stack_graph.source_info_mut(node_handle);
        source_info.syntax_type = syntax_type.into();

        let mut map_namespace_nodes: HashMap<String, Handle<Node>> = HashMap::new();
        let mut map_class_nodes: HashMap<String, Handle<Node>> = HashMap::new();
        let mut map_method_nodes: HashMap<String, Handle<Node>> = HashMap::new();
        let mut map_field_nodes: HashMap<String, Handle<Node>> = HashMap::new();

        let mut node_tracking_number = 0;
        for node in inter_node_info {
            let id: Handle<Node> = match node.syntax_type {
                SyntaxType::FieldName => {
                    let node_id = map_field_nodes.get(&node.symbol);
                    if node_id.is_none() {
                        let id = stack_graph.new_node_id(file);
                        let symbol = stack_graph.add_symbol(&node.symbol);
                        let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                        if node_handle.is_none() {
                            info!("node_handle is none???");
                            continue;
                        }
                        let node_handle = node_handle.unwrap();
                        map_field_nodes.insert(node.symbol.clone(), node_handle);
                        node_handle
                    } else {
                        continue;
                    }
                }
                SyntaxType::ClassDef => {
                    let node_id = map_class_nodes.get(&node.symbol);
                    if node_id.is_none() {
                        let id = stack_graph.new_node_id(file);
                        let symbol = stack_graph.add_symbol(&node.symbol);
                        let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                        if node_handle.is_none() {
                            info!("node_handle is none???");
                            continue;
                        }
                        let node_handle = node_handle.unwrap();
                        map_class_nodes.insert(node.symbol.clone(), node_handle);
                        node_handle
                    } else {
                        continue;
                    }
                }
                SyntaxType::MethodName => {
                    let node_id = map_method_nodes.get(&node.symbol);
                    if node_id.is_none() {
                        let id = stack_graph.new_node_id(file);
                        let symbol = stack_graph.add_symbol(&node.symbol);
                        let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                        if node_handle.is_none() {
                            info!("node_handle is none???");
                            continue;
                        }
                        let node_handle = node_handle.unwrap();
                        map_method_nodes.insert(node.symbol.clone(), node_handle);
                        node_handle
                    } else {
                        continue;
                    }
                }
                SyntaxType::NamespaceDeclaration => {
                    let node_id = map_namespace_nodes.get(&node.symbol);
                    if node_id.is_none() {
                        let id = stack_graph.new_node_id(file);
                        let symbol = stack_graph.add_symbol(&node.symbol);
                        let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                        if node_handle.is_none() {
                            info!("node_handle is none???");
                            continue;
                        }
                        let node_handle = node_handle.unwrap();
                        map_namespace_nodes.insert(node.symbol.clone(), node_handle);
                        node_handle
                    } else {
                        continue;
                    }
                }
                _ => {
                    error!("unable to get node syntax type");
                    return Err(BuildError::ParseError);
                }
            };
            let syntax_type = stack_graph.add_string(&node.syntax_type.to_string());
            let source_info = stack_graph.source_info_mut(id);
            source_info.syntax_type = syntax_type.into();
            node_tracking_number += 1
        }

        let mut edge_tracking_number = 0;
        for edge in inter_edge_info {
            let source_graph_node = match edge.source.syntax_type {
                SyntaxType::FieldName => {
                    let graph_node = map_field_nodes.get(&edge.source.symbol);
                    if graph_node.is_none() {
                        error!("didn't create graph node for field {:?}", edge);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::ClassDef => {
                    let graph_node = map_class_nodes.get(&edge.source.symbol);
                    if graph_node.is_none() {
                        error!("didn't create graph node for field {:?}", edge.source);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::MethodName => {
                    let graph_node = map_method_nodes.get(&edge.source.symbol);
                    if graph_node.is_none() {
                        error!("didn't create graph node for field {:?}", edge.source);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::NamespaceDeclaration => {
                    let graph_node = map_namespace_nodes.get(&edge.source.symbol);
                    if graph_node.is_none() {
                        error!("didn't create graph node for field {:?}", edge.source);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                _ => {
                    error!("uanble to get node syntax type");
                    return Err(BuildError::UnknownNodeType(format!(
                        "unable to get edge source symbol: {:?}",
                        edge,
                    )));
                }
            };
            let sink_graph_node = match edge.sink.syntax_type {
                SyntaxType::FieldName => {
                    let graph_node = map_field_nodes.get(&edge.sink.symbol);
                    if graph_node.is_none() {
                        error!("didn't create graph node for field {:?}", edge.sink);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::ClassDef => {
                    let graph_node = map_class_nodes.get(&edge.sink.symbol);
                    if graph_node.is_none() {
                        error!("didn't create graph node for class {:?} sink", edge.sink);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::MethodName => {
                    let graph_node = map_method_nodes.get(&edge.sink.symbol);
                    if graph_node.is_none() {
                        error!("didn't create graph node for field {:?}", edge.sink);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::NamespaceDeclaration => {
                    let graph_node = map_namespace_nodes.get(&edge.sink.symbol);
                    if graph_node.is_none() {
                        error!("didn't create graph node for field {:?}", edge.sink);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                _ => {
                    error!("didn't create graph node for field {:?}", edge);
                    return Err(BuildError::ParseError);
                }
            };
            stack_graph.add_edge(*source_graph_node, *sink_graph_node, edge.precedence);
            edge_tracking_number += 1;
        }

        info!(
            "created {} graph nodes {} edge nodes",
            &node_tracking_number, &edge_tracking_number
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
                    syntax_type: SyntaxType::NamespaceDeclaration,
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
                    syntax_type: SyntaxType::ClassDef,
                };
                nodes.push(type_name.clone());
                let namespace_symbol = parts.fold("".to_string(), |acc, p| {
                    if acc.is_empty() {
                        p.to_string()
                    } else {
                        format!("{}.{}", acc, p)
                    }
                });
                let namesapce_node = NodeInfo {
                    symbol: namespace_symbol.clone(),
                    syntax_type: SyntaxType::NamespaceDeclaration,
                };
                nodes.push(namesapce_node.clone());
                edges.push(EdgeInfo {
                    source: namesapce_node.clone(),
                    sink: type_name.clone(),
                    precedence: 0,
                });
                edges.push(EdgeInfo {
                    source: type_name,
                    sink: namesapce_node,
                    precedence: 10,
                });
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
                let field_name = NodeInfo {
                    symbol: part.unwrap().to_string(),
                    syntax_type: SyntaxType::FieldName,
                };
                nodes.push(field_name.clone());
                let part = parts.next_back();
                if part.is_none() {
                    return (vec![], vec![]);
                }
                let type_name = NodeInfo {
                    symbol: part.unwrap().to_string(),
                    syntax_type: SyntaxType::ClassDef,
                };
                nodes.push(type_name.clone());
                let namespace_symbol = parts.fold("".to_string(), |acc, p| {
                    if acc.is_empty() {
                        p.to_string()
                    } else {
                        format!("{}.{}", acc, p)
                    }
                });
                let namesapce_node = NodeInfo {
                    symbol: namespace_symbol.clone(),
                    syntax_type: SyntaxType::NamespaceDeclaration,
                };
                nodes.push(namesapce_node.clone());
                edges.push(EdgeInfo {
                    source: namesapce_node.clone(),
                    sink: type_name.clone(),
                    precedence: 0,
                });
                edges.push(EdgeInfo {
                    source: type_name.clone(),
                    sink: field_name.clone(),
                    precedence: 0,
                });
                edges.push(EdgeInfo {
                    source: field_name,
                    sink: type_name.clone(),
                    precedence: 10,
                });
                edges.push(EdgeInfo {
                    source: type_name.clone(),
                    sink: namesapce_node,
                    precedence: 10,
                });
                (nodes, edges)
            }
            "M" => {
                let mut new_name = name;
                if name.contains('(') {
                    let mut x = name.split('(');
                    let x = x.nth(0);
                    new_name = x.unwrap();
                }
                debug!("method new string to deal with: {}", new_name);
                let mut parts = new_name.split('.');
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
                let method_node: NodeInfo;
                let type_name: NodeInfo;
                if part.contains("#ctor") {
                    // Get the next back Symbol and that will be the symbol.
                    let part = parts.next_back();
                    if part.is_none() {
                        return (vec![], vec![]);
                    }
                    method_node = NodeInfo {
                        symbol: part.unwrap().to_string(),
                        syntax_type: SyntaxType::MethodName,
                    };
                    type_name = NodeInfo {
                        symbol: part.unwrap().to_string(),
                        syntax_type: SyntaxType::ClassDef,
                    };
                } else {
                    method_node = NodeInfo {
                        symbol: part.to_string(),
                        syntax_type: SyntaxType::MethodName,
                    };
                    let part = parts.next_back();
                    if part.is_none() {
                        return (vec![], vec![]);
                    }
                    type_name = NodeInfo {
                        symbol: part.unwrap().to_string(),
                        syntax_type: SyntaxType::ClassDef,
                    };
                };
                nodes.push(method_node.clone());
                nodes.push(type_name.clone());
                let namespace_symbol = parts.fold("".to_string(), |acc, p| {
                    if acc.is_empty() {
                        p.to_string()
                    } else {
                        format!("{}.{}", acc, p)
                    }
                });
                let namesapce_node = NodeInfo {
                    symbol: namespace_symbol.clone(),
                    syntax_type: SyntaxType::NamespaceDeclaration,
                };
                nodes.push(namesapce_node.clone());
                edges.push(EdgeInfo {
                    source: namesapce_node.clone(),
                    sink: type_name.clone(),
                    precedence: 0,
                });
                edges.push(EdgeInfo {
                    source: type_name.clone(),
                    sink: method_node.clone(),
                    precedence: 0,
                });
                edges.push(EdgeInfo {
                    source: method_node,
                    sink: type_name.clone(),
                    precedence: 10,
                });
                edges.push(EdgeInfo {
                    source: type_name.clone(),
                    sink: namesapce_node,
                    precedence: 10,
                });
                (nodes, edges)
            }
            _ => {
                info!("unable to handle: {} -- {}", member_type, name);
                (vec![], vec![])
            }
        }
    }
}
