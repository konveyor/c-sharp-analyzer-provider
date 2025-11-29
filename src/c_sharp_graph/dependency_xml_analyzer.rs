use std::collections::HashMap;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    symbol: String,
    syntax_type: SyntaxType,
}

#[derive(Debug, PartialEq, Eq)]
pub struct EdgeInfo {
    source: NodeInfo,
    sink: NodeInfo,
    precedence: i32,
}

impl FileAnalyzer for DepXMLFileAnalyzer {
    #[allow(clippy::needless_lifetimes)]
    fn build_stack_graph_into<'a>(
        &self,
        stack_graph: &mut StackGraph,
        file: Handle<File>,
        path: &Path,
        source: &str,
        _all_paths: &mut dyn Iterator<Item = &'a Path>,
        _globals: &HashMap<String, String>,
        _cancellation_flag: &dyn CancellationFlag,
    ) -> Result<(), tree_sitter_stack_graphs::BuildError> {
        let mut reader = Reader::from_str(source);

        reader.config_mut().trim_text(true);

        let mut inter_node_info: Vec<NodeInfo> = vec![];
        let mut inter_edge_info: Vec<EdgeInfo> = vec![];
        loop {
            match reader.read_event() {
                Err(e) => {
                    error!(file=?path, "got errror {}", e);
                    return Err(BuildError::ParseError);
                }
                Ok(Event::Eof) => {
                    break;
                }
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    if e.name() == MEMBER_NAME {
                        // Look specifically for the "name" attribute for deterministic behavior
                        let member_name = e.attributes().find(|attr| match attr {
                            Ok(a) => a.key == QName(b"name"),
                            Err(_) => false,
                        });
                        if member_name.is_none() {
                            continue;
                        }
                        let member_name = member_name.unwrap().unwrap();
                        let member_name = String::from_utf8_lossy(&member_name.value).to_string();
                        let parts: Vec<&str> = member_name.split(":").collect();
                        if parts.len() != 2 {
                            debug!(file=?path, "unable to get correct parts: {}", &member_name);
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
            file=?path,
            "got {} nodes and {} edges to be created",
            &inter_node_info.len(),
            &inter_edge_info.len()
        );

        // Create Compilation Unit.
        let id = stack_graph.new_node_id(file);
        let symbol = stack_graph.add_symbol(path.to_string_lossy().as_ref());
        let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
        if node_handle.is_none() {
            error!(file=?path, "node_handle is none???");
            return Err(BuildError::UnknownSymbolType(
                "unable to handle comp unit".to_string(),
            ));
        }
        let comp_unit_node_handle = node_handle.unwrap();
        let syntax_type = stack_graph.add_string(SyntaxType::CompUnit.to_string());
        let source_info = stack_graph.source_info_mut(comp_unit_node_handle);
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
                            continue;
                        }
                        let node_handle = node_handle.unwrap();
                        map_namespace_nodes.insert(node.symbol.clone(), node_handle);

                        stack_graph.add_edge(comp_unit_node_handle, node_handle, 0);
                        node_handle
                    } else {
                        continue;
                    }
                }
                _ => {
                    error!(file = ?path, "unable to get node syntax type");
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
                        error!(file=?path, "didn't create graph node for field {:?}", edge);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::ClassDef => {
                    let graph_node = map_class_nodes.get(&edge.source.symbol);
                    if graph_node.is_none() {
                        error!(file=?path, "didn't create graph node for field {:?}", edge.source);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::MethodName => {
                    let graph_node = map_method_nodes.get(&edge.source.symbol);
                    if graph_node.is_none() {
                        error!(file=?path, "didn't create graph node for field {:?}", edge.source);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::NamespaceDeclaration => {
                    let graph_node = map_namespace_nodes.get(&edge.source.symbol);
                    if graph_node.is_none() {
                        error!(file=?path, "didn't create graph node for field {:?}", edge.source);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                _ => {
                    error!(file=?path, "uanble to get node syntax type");
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
                        error!(file=?path, "didn't create graph node for field {:?}", edge.sink);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::ClassDef => {
                    let graph_node = map_class_nodes.get(&edge.sink.symbol);
                    if graph_node.is_none() {
                        error!(file=?path, "didn't create graph node for class {:?} sink", edge.sink);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::MethodName => {
                    let graph_node = map_method_nodes.get(&edge.sink.symbol);
                    if graph_node.is_none() {
                        error!(file=?path, "didn't create graph node for field {:?}", edge.sink);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                SyntaxType::NamespaceDeclaration => {
                    let graph_node = map_namespace_nodes.get(&edge.sink.symbol);
                    if graph_node.is_none() {
                        error!(file=?path, "didn't create graph node for field {:?}", edge.sink);
                        return Err(BuildError::ParseError);
                    }
                    graph_node.unwrap()
                }
                _ => {
                    error!(file=?path, "didn't create graph node for field {:?}", edge);
                    return Err(BuildError::ParseError);
                }
            };
            stack_graph.add_edge(*source_graph_node, *sink_graph_node, edge.precedence);
            edge_tracking_number += 1;
        }

        info!(
            file=?path,
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
                if name.is_empty() {
                    return (vec![], vec![]);
                }
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
                    let interface_check_parts: Vec<&str> = p.split("#").collect();
                    let t = if interface_check_parts.len() > 1 {
                        interface_check_parts[0]
                    } else {
                        p
                    };

                    if acc.is_empty() {
                        t.to_string()
                    } else {
                        format!("{}.{}", acc, t)
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
                if name.is_empty() {
                    return (vec![], vec![]);
                }
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
                if name.is_empty() {
                    return (vec![], vec![]);
                }
                let mut new_name = name;
                if name.contains('(') {
                    let mut x = name.split('(');
                    let x = x.nth(0);
                    new_name = x.unwrap();
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create a DepXMLFileAnalyzer for testing
    fn create_analyzer() -> DepXMLFileAnalyzer {
        DepXMLFileAnalyzer {}
    }

    // Tests for Namespace (N) type

    #[test]
    fn test_handle_member_namespace_simple() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("N", "System");

        assert_eq!(nodes.len(), 1);
        assert_eq!(edges.len(), 0);

        assert_eq!(nodes[0].symbol, "System");
        assert_eq!(nodes[0].syntax_type, SyntaxType::NamespaceDeclaration);
    }

    #[test]
    fn test_handle_member_namespace_nested() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("N", "System.Configuration");

        assert_eq!(nodes.len(), 1);
        assert_eq!(edges.len(), 0);

        // Namespace member type returns the full name as-is
        assert_eq!(nodes[0].symbol, "System.Configuration");
        assert_eq!(nodes[0].syntax_type, SyntaxType::NamespaceDeclaration);
    }

    #[test]
    fn test_handle_member_namespace_empty() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("N", "");

        assert_eq!(nodes.len(), 1);
        assert_eq!(edges.len(), 0);
        assert_eq!(nodes[0].symbol, "");
    }

    // Tests for Type (T) member

    #[test]
    fn test_handle_member_type_simple() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("T", "System.String");

        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 2);

        // First node should be the class
        assert_eq!(nodes[0].symbol, "String");
        assert_eq!(nodes[0].syntax_type, SyntaxType::ClassDef);

        // Second node should be the namespace
        assert_eq!(nodes[1].symbol, "System");
        assert_eq!(nodes[1].syntax_type, SyntaxType::NamespaceDeclaration);

        // Check edges
        // Edge 1: namespace -> class (precedence 0)
        assert_eq!(edges[0].source.symbol, "System");
        assert_eq!(edges[0].sink.symbol, "String");
        assert_eq!(edges[0].precedence, 0);

        // Edge 2: class -> namespace (precedence 10, for FQDN traversal)
        assert_eq!(edges[1].source.symbol, "String");
        assert_eq!(edges[1].sink.symbol, "System");
        assert_eq!(edges[1].precedence, 10);
    }

    #[test]
    fn test_handle_member_type_nested_namespace() {
        let analyzer = create_analyzer();
        let (nodes, edges) =
            analyzer.handle_member("T", "System.Configuration.ConfigurationManager");

        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 2);

        // Class name
        assert_eq!(nodes[0].symbol, "ConfigurationManager");
        assert_eq!(nodes[0].syntax_type, SyntaxType::ClassDef);

        // Namespace (should be concatenated)
        assert_eq!(nodes[1].symbol, "System.Configuration");
        assert_eq!(nodes[1].syntax_type, SyntaxType::NamespaceDeclaration);
    }

    #[test]
    fn test_handle_member_type_no_namespace() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("T", "String");

        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 2);

        assert_eq!(nodes[0].symbol, "String");
        assert_eq!(nodes[1].symbol, ""); // Empty namespace
    }

    #[test]
    fn test_handle_member_type_empty_string() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("T", "");

        // Empty string should return empty vectors
        assert_eq!(nodes.len(), 0);
        assert_eq!(edges.len(), 0);
    }

    // Tests for Field (F) and Property (P) members

    #[test]
    fn test_handle_member_field_simple() {
        let analyzer = create_analyzer();
        let (nodes, edges) =
            analyzer.handle_member("F", "System.Configuration.ConfigurationManager.AppSettings");

        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 4);

        // Field node
        assert_eq!(nodes[0].symbol, "AppSettings");
        assert_eq!(nodes[0].syntax_type, SyntaxType::FieldName);

        // Class node
        assert_eq!(nodes[1].symbol, "ConfigurationManager");
        assert_eq!(nodes[1].syntax_type, SyntaxType::ClassDef);

        // Namespace node
        assert_eq!(nodes[2].symbol, "System.Configuration");
        assert_eq!(nodes[2].syntax_type, SyntaxType::NamespaceDeclaration);

        // Check edges
        // Edge 0: namespace -> class (precedence 0)
        assert_eq!(edges[0].source.symbol, "System.Configuration");
        assert_eq!(edges[0].sink.symbol, "ConfigurationManager");
        assert_eq!(edges[0].precedence, 0);

        // Edge 1: class -> field (precedence 0)
        assert_eq!(edges[1].source.symbol, "ConfigurationManager");
        assert_eq!(edges[1].sink.symbol, "AppSettings");
        assert_eq!(edges[1].precedence, 0);

        // Edge 2: field -> class (precedence 10, FQDN)
        assert_eq!(edges[2].source.symbol, "AppSettings");
        assert_eq!(edges[2].sink.symbol, "ConfigurationManager");
        assert_eq!(edges[2].precedence, 10);

        // Edge 3: class -> namespace (precedence 10, FQDN)
        assert_eq!(edges[3].source.symbol, "ConfigurationManager");
        assert_eq!(edges[3].sink.symbol, "System.Configuration");
        assert_eq!(edges[3].precedence, 10);
    }

    #[test]
    fn test_handle_member_property_same_as_field() {
        let analyzer = create_analyzer();
        let (nodes_f, edges_f) = analyzer.handle_member("F", "System.Console.Out");
        let (nodes_p, edges_p) = analyzer.handle_member("P", "System.Console.Out");

        // Field and Property should be handled identically
        assert_eq!(nodes_f.len(), nodes_p.len());
        assert_eq!(edges_f.len(), edges_p.len());

        assert_eq!(nodes_f[0].symbol, nodes_p[0].symbol);
        assert_eq!(nodes_f[0].syntax_type, nodes_p[0].syntax_type);
    }

    #[test]
    fn test_handle_member_field_missing_parts() {
        let analyzer = create_analyzer();

        // Only class and field, no namespace
        let (nodes, _edges) = analyzer.handle_member("F", "Console.Out");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[2].symbol, ""); // Empty namespace

        // Only one part - should return empty
        let (nodes, edges) = analyzer.handle_member("F", "Out");
        assert_eq!(nodes.len(), 0);
        assert_eq!(edges.len(), 0);

        // Empty string
        let (nodes, edges) = analyzer.handle_member("F", "");
        assert_eq!(nodes.len(), 0);
        assert_eq!(edges.len(), 0);
    }

    // Tests for Method (M) members

    #[test]
    fn test_handle_member_method_simple() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("M", "System.String.Format");

        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 4);

        // Method node
        assert_eq!(nodes[0].symbol, "Format");
        assert_eq!(nodes[0].syntax_type, SyntaxType::MethodName);

        // Class node
        assert_eq!(nodes[1].symbol, "String");
        assert_eq!(nodes[1].syntax_type, SyntaxType::ClassDef);

        // Namespace node
        assert_eq!(nodes[2].symbol, "System");
        assert_eq!(nodes[2].syntax_type, SyntaxType::NamespaceDeclaration);
    }

    #[test]
    fn test_handle_member_method_with_parameters() {
        let analyzer = create_analyzer();
        let (nodes, _edges) =
            analyzer.handle_member("M", "System.String.Format(System.String,System.Object)");

        assert_eq!(nodes.len(), 3);

        // Method name should be extracted without parameters
        assert_eq!(nodes[0].symbol, "Format");
        assert_eq!(nodes[0].syntax_type, SyntaxType::MethodName);

        assert_eq!(nodes[1].symbol, "String");
        assert_eq!(nodes[2].symbol, "System");
    }

    #[test]
    fn test_handle_member_method_with_complex_parameters() {
        let analyzer = create_analyzer();
        let (nodes, _) = analyzer.handle_member(
            "M",
            "System.Collections.Generic.List.Add(System.Collections.Generic.T)",
        );

        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].symbol, "Add");
        assert_eq!(nodes[1].symbol, "List");
        assert_eq!(nodes[2].symbol, "System.Collections.Generic");
    }

    #[test]
    fn test_handle_member_method_constructor() {
        let analyzer = create_analyzer();
        let (nodes, _edges) = analyzer.handle_member("M", "System.String.#ctor");

        assert_eq!(nodes.len(), 3);

        // For constructor, both method and class should use the class name
        assert_eq!(nodes[0].symbol, "String");
        assert_eq!(nodes[0].syntax_type, SyntaxType::MethodName);

        assert_eq!(nodes[1].symbol, "String");
        assert_eq!(nodes[1].syntax_type, SyntaxType::ClassDef);

        assert_eq!(nodes[2].symbol, "System");
        assert_eq!(nodes[2].syntax_type, SyntaxType::NamespaceDeclaration);
    }

    #[test]
    fn test_handle_member_method_constructor_with_params() {
        let analyzer = create_analyzer();
        let (nodes, _) = analyzer.handle_member("M", "System.String.#ctor(System.Char[])");

        assert_eq!(nodes.len(), 3);

        // Constructor with parameters - params should be stripped
        assert_eq!(nodes[0].symbol, "String");
        assert_eq!(nodes[1].symbol, "String");
        assert_eq!(nodes[2].symbol, "System");
    }

    #[test]
    fn test_handle_member_method_nested_namespace() {
        let analyzer = create_analyzer();
        let (nodes, _) =
            analyzer.handle_member("M", "System.Configuration.ConfigurationManager.GetSection");

        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].symbol, "GetSection");
        assert_eq!(nodes[1].symbol, "ConfigurationManager");
        assert_eq!(nodes[2].symbol, "System.Configuration");
    }

    #[test]
    fn test_handle_member_method_missing_parts() {
        let analyzer = create_analyzer();

        // Only class and method, no namespace
        let (nodes, _) = analyzer.handle_member("M", "String.Format");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[2].symbol, ""); // Empty namespace

        // Only one part - should return empty
        let (nodes, edges) = analyzer.handle_member("M", "Format");
        assert_eq!(nodes.len(), 0);
        assert_eq!(edges.len(), 0);

        // Empty string
        let (nodes, edges) = analyzer.handle_member("M", "");
        assert_eq!(nodes.len(), 0);
        assert_eq!(edges.len(), 0);
    }

    #[test]
    fn test_handle_member_method_constructor_missing_class() {
        let analyzer = create_analyzer();
        // Constructor with no class name before it
        let (nodes, edges) = analyzer.handle_member("M", "#ctor");

        assert_eq!(nodes.len(), 0);
        assert_eq!(edges.len(), 0);
    }

    // Tests for unknown member types

    #[test]
    fn test_handle_member_unknown_type() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("X", "System.Something");

        assert_eq!(nodes.len(), 0);
        assert_eq!(edges.len(), 0);
    }

    #[test]
    fn test_handle_member_empty_type() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("", "System.Something");

        assert_eq!(nodes.len(), 0);
        assert_eq!(edges.len(), 0);
    }

    // Edge precedence tests

    #[test]
    fn test_edge_precedence_values() {
        let analyzer = create_analyzer();

        // Test Type edges
        let (_, edges) = analyzer.handle_member("T", "System.String");
        assert_eq!(edges[0].precedence, 0); // namespace -> class
        assert_eq!(edges[1].precedence, 10); // class -> namespace (FQDN)

        // Test Field edges
        let (_, edges) = analyzer.handle_member("F", "System.Console.Out");
        assert_eq!(edges[0].precedence, 0); // namespace -> class
        assert_eq!(edges[1].precedence, 0); // class -> field
        assert_eq!(edges[2].precedence, 10); // field -> class (FQDN)
        assert_eq!(edges[3].precedence, 10); // class -> namespace (FQDN)

        // Test Method edges
        let (_, edges) = analyzer.handle_member("M", "System.String.Format");
        assert_eq!(edges[0].precedence, 0); // namespace -> class
        assert_eq!(edges[1].precedence, 0); // class -> method
        assert_eq!(edges[2].precedence, 10); // method -> class (FQDN)
        assert_eq!(edges[3].precedence, 10); // class -> namespace (FQDN)
    }

    // Integration tests with real-world examples

    #[test]
    fn test_handle_member_real_world_configuration_manager() {
        let analyzer = create_analyzer();
        let (nodes, edges) =
            analyzer.handle_member("F", "System.Configuration.ConfigurationManager.AppSettings");

        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 4);

        // Verify the complete graph structure
        let field = &nodes[0];
        let class = &nodes[1];
        let namespace = &nodes[2];

        assert_eq!(field.symbol, "AppSettings");
        assert_eq!(class.symbol, "ConfigurationManager");
        assert_eq!(namespace.symbol, "System.Configuration");

        // Verify FQDN can be built: field -> class -> namespace
        assert_eq!(edges[2].source.symbol, field.symbol);
        assert_eq!(edges[2].sink.symbol, class.symbol);
        assert_eq!(edges[3].source.symbol, class.symbol);
        assert_eq!(edges[3].sink.symbol, namespace.symbol);
    }

    #[test]
    fn test_handle_member_real_world_linq_method() {
        let analyzer = create_analyzer();
        let (nodes, _) = analyzer.handle_member(
            "M",
            "System.Linq.Enumerable.Where(System.Collections.Generic.IEnumerable,System.Func)",
        );

        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].symbol, "Where");
        assert_eq!(nodes[1].symbol, "Enumerable");
        assert_eq!(nodes[2].symbol, "System.Linq");
    }

    #[test]
    fn test_handle_member_real_world_type_with_generics() {
        let analyzer = create_analyzer();
        let (nodes, _) = analyzer.handle_member("T", "System.Collections.Generic.List`1");

        assert_eq!(nodes.len(), 2);
        // Generic type notation is preserved
        assert_eq!(nodes[0].symbol, "List`1");
        assert_eq!(nodes[1].symbol, "System.Collections.Generic");
    }

    // FQDN Integration Tests
    // These tests verify that nodes/edges from handle_member() produce correct FQDNs

    use crate::c_sharp_graph::query::get_fqdn;
    use stack_graphs::graph::StackGraph;
    use std::collections::HashMap;

    /// Helper function to build a stack graph from NodeInfo and EdgeInfo structures
    fn build_stack_graph_from_nodes_edges(
        nodes: Vec<NodeInfo>,
        edges: Vec<EdgeInfo>,
    ) -> (StackGraph, HashMap<String, Handle<Node>>) {
        let mut graph = StackGraph::new();
        let file = graph.add_file("test.cs").unwrap();

        let mut node_map: HashMap<String, Handle<Node>> = HashMap::new();

        // Create all nodes
        for node_info in nodes {
            let key = format!("{:?}:{}", node_info.syntax_type, node_info.symbol);

            // Skip duplicates
            if node_map.contains_key(&key) {
                continue;
            }

            let node_id = graph.new_node_id(file);
            let symbol = graph.add_symbol(&node_info.symbol);
            let node_handle = graph.add_pop_symbol_node(node_id, symbol, true).unwrap();

            // Set syntax type
            let syntax_type_str = graph.add_string(node_info.syntax_type.to_string());
            let source_info = graph.source_info_mut(node_handle);
            source_info.syntax_type = syntax_type_str.into();

            node_map.insert(key, node_handle);
        }

        // Create all edges
        for edge_info in edges {
            let source_key = format!(
                "{:?}:{}",
                edge_info.source.syntax_type, edge_info.source.symbol
            );
            let sink_key = format!("{:?}:{}", edge_info.sink.syntax_type, edge_info.sink.symbol);

            let source_handle = node_map
                .get(&source_key)
                .unwrap_or_else(|| panic!("Source node not found: {}", source_key));
            let sink_handle = node_map
                .get(&sink_key)
                .unwrap_or_else(|| panic!("Sink node not found: {}", sink_key));

            graph.add_edge(*source_handle, *sink_handle, edge_info.precedence);
        }

        (graph, node_map)
    }

    #[test]
    fn test_fqdn_for_type() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("T", "System.String");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the class node (String)
        let class_key = format!("{:?}:String", SyntaxType::ClassDef);
        let class_handle = node_map.get(&class_key).unwrap();

        let fqdn = get_fqdn(*class_handle, &graph).unwrap();

        assert_eq!(fqdn.namespace, Some("System".to_string()));
        assert_eq!(fqdn.class, Some("String".to_string()));
        assert_eq!(fqdn.method, None);
        assert_eq!(fqdn.field, None);
    }

    #[test]
    fn test_fqdn_for_field() {
        let analyzer = create_analyzer();
        let (nodes, edges) =
            analyzer.handle_member("F", "System.Configuration.ConfigurationManager.AppSettings");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the field node (AppSettings)
        let field_key = format!("{:?}:AppSettings", SyntaxType::FieldName);
        let field_handle = node_map.get(&field_key).unwrap();

        let fqdn = get_fqdn(*field_handle, &graph).unwrap();

        assert_eq!(fqdn.namespace, Some("System.Configuration".to_string()));
        assert_eq!(fqdn.class, Some("ConfigurationManager".to_string()));
        assert_eq!(fqdn.field, Some("AppSettings".to_string()));
        assert_eq!(fqdn.method, None);
    }

    #[test]
    fn test_fqdn_for_method() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("M", "System.String.Format");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the method node (Format)
        let method_key = format!("{:?}:Format", SyntaxType::MethodName);
        let method_handle = node_map.get(&method_key).unwrap();

        let fqdn = get_fqdn(*method_handle, &graph).unwrap();

        assert_eq!(fqdn.namespace, Some("System".to_string()));
        assert_eq!(fqdn.class, Some("String".to_string()));
        assert_eq!(fqdn.method, Some("Format".to_string()));
        assert_eq!(fqdn.field, None);
    }

    #[test]
    fn test_fqdn_for_method_with_parameters() {
        let analyzer = create_analyzer();
        let (nodes, edges) =
            analyzer.handle_member("M", "System.String.Format(System.String,System.Object)");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the method node (Format) - parameters should be stripped
        let method_key = format!("{:?}:Format", SyntaxType::MethodName);
        let method_handle = node_map.get(&method_key).unwrap();

        let fqdn = get_fqdn(*method_handle, &graph).unwrap();

        assert_eq!(fqdn.namespace, Some("System".to_string()));
        assert_eq!(fqdn.class, Some("String".to_string()));
        assert_eq!(fqdn.method, Some("Format".to_string()));
        assert_eq!(fqdn.field, None);
    }

    #[test]
    fn test_fqdn_for_constructor() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("M", "System.String.#ctor");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the method node (String constructor)
        let method_key = format!("{:?}:String", SyntaxType::MethodName);
        let method_handle = node_map.get(&method_key).unwrap();

        let fqdn = get_fqdn(*method_handle, &graph).unwrap();

        assert_eq!(fqdn.namespace, Some("System".to_string()));
        assert_eq!(fqdn.class, Some("String".to_string()));
        assert_eq!(fqdn.method, Some("String".to_string()));
        assert_eq!(fqdn.field, None);
    }

    #[test]
    fn test_fqdn_for_nested_namespace_type() {
        let analyzer = create_analyzer();
        let (nodes, edges) =
            analyzer.handle_member("T", "System.Configuration.ConfigurationManager");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the class node
        let class_key = format!("{:?}:ConfigurationManager", SyntaxType::ClassDef);
        let class_handle = node_map.get(&class_key).unwrap();

        let fqdn = get_fqdn(*class_handle, &graph).unwrap();

        assert_eq!(fqdn.namespace, Some("System.Configuration".to_string()));
        assert_eq!(fqdn.class, Some("ConfigurationManager".to_string()));
        assert_eq!(fqdn.method, None);
        assert_eq!(fqdn.field, None);
    }

    #[test]
    fn test_fqdn_for_nested_namespace_method() {
        let analyzer = create_analyzer();
        let (nodes, edges) =
            analyzer.handle_member("M", "System.Configuration.ConfigurationManager.GetSection");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the method node
        let method_key = format!("{:?}:GetSection", SyntaxType::MethodName);
        let method_handle = node_map.get(&method_key).unwrap();

        let fqdn = get_fqdn(*method_handle, &graph).unwrap();

        assert_eq!(fqdn.namespace, Some("System.Configuration".to_string()));
        assert_eq!(fqdn.class, Some("ConfigurationManager".to_string()));
        assert_eq!(fqdn.method, Some("GetSection".to_string()));
        assert_eq!(fqdn.field, None);
    }

    #[test]
    fn test_fqdn_for_linq_method() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member(
            "M",
            "System.Linq.Enumerable.Where(System.Collections.Generic.IEnumerable,System.Func)",
        );

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the method node
        let method_key = format!("{:?}:Where", SyntaxType::MethodName);
        let method_handle = node_map.get(&method_key).unwrap();

        let fqdn = get_fqdn(*method_handle, &graph).unwrap();

        assert_eq!(fqdn.namespace, Some("System.Linq".to_string()));
        assert_eq!(fqdn.class, Some("Enumerable".to_string()));
        assert_eq!(fqdn.method, Some("Where".to_string()));
        assert_eq!(fqdn.field, None);
    }

    #[test]
    fn test_fqdn_for_property() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("P", "System.Console.Out");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the property/field node
        let field_key = format!("{:?}:Out", SyntaxType::FieldName);
        let field_handle = node_map.get(&field_key).unwrap();

        let fqdn = get_fqdn(*field_handle, &graph).unwrap();

        assert_eq!(fqdn.namespace, Some("System".to_string()));
        assert_eq!(fqdn.class, Some("Console".to_string()));
        assert_eq!(fqdn.field, Some("Out".to_string()));
        assert_eq!(fqdn.method, None);
    }

    #[test]
    fn test_fqdn_from_class_node() {
        let analyzer = create_analyzer();
        let (nodes, edges) =
            analyzer.handle_member("F", "System.Configuration.ConfigurationManager.AppSettings");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the class node instead of field
        let class_key = format!("{:?}:ConfigurationManager", SyntaxType::ClassDef);
        let class_handle = node_map.get(&class_key).unwrap();

        let fqdn = get_fqdn(*class_handle, &graph).unwrap();

        // Should only have namespace and class, not field
        assert_eq!(fqdn.namespace, Some("System.Configuration".to_string()));
        assert_eq!(fqdn.class, Some("ConfigurationManager".to_string()));
        assert_eq!(fqdn.field, None);
        assert_eq!(fqdn.method, None);
    }

    #[test]
    fn test_fqdn_from_namespace_node() {
        let analyzer = create_analyzer();
        let (nodes, edges) = analyzer.handle_member("T", "System.String");

        let (graph, node_map) = build_stack_graph_from_nodes_edges(nodes, edges);

        // Get FQDN from the namespace node
        let namespace_key = format!("{:?}:System", SyntaxType::NamespaceDeclaration);
        let namespace_handle = node_map.get(&namespace_key).unwrap();

        let fqdn = get_fqdn(*namespace_handle, &graph).unwrap();

        // Should only have namespace
        assert_eq!(fqdn.namespace, Some("System".to_string()));
        assert_eq!(fqdn.class, None);
        assert_eq!(fqdn.field, None);
        assert_eq!(fqdn.method, None);
    }
}
