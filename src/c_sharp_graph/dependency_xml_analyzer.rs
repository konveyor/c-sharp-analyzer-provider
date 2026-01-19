use std::collections::HashMap;
use std::iter::DoubleEndedIterator;
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

        let mut inter_node_info: Vec<Vec<NodeInfo>> = vec![];
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
                        let nodes =
                            self.handle_member(parts.first().unwrap(), parts.last().unwrap());
                        inter_node_info.push(nodes);
                    }
                    continue;
                }
                _ => (),
            }
        }
        info!(
            file=?path,
            "got {} nodes to be created",
            &inter_node_info.len(),
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

        let mut node_tracking_number = 0;
        let mut edge_tracking_number = 0;
        let mut namespace_node_map: HashMap<String, Handle<Node>> = HashMap::new();
        let mut type_node_map: HashMap<String, Handle<Node>> = HashMap::new();
        // Anything under a type, needs to be scoped to that type,
        // And because we don't have a good way of de-duping here
        // We will just create more nodes.
        for nodes in inter_node_info {
            match nodes.len() {
                1 => {
                    let namespace_node = nodes.first();
                    if namespace_node.is_none() {
                        continue;
                    }
                    let namespace_node = namespace_node.unwrap();
                    // If the list is size of one, then we only have the namespace declartion.
                    if namespace_node_map.contains_key(&namespace_node.symbol) {
                        continue;
                    }
                    let id = stack_graph.new_node_id(file);
                    let symbol = stack_graph.add_symbol(&namespace_node.symbol);
                    let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                    if node_handle.is_none() {
                        continue;
                    }
                    let node_handle = node_handle.unwrap();
                    let syntax_type =
                        stack_graph.add_string(&namespace_node.syntax_type.to_string());
                    let source_info = stack_graph.source_info_mut(node_handle);
                    source_info.syntax_type = syntax_type.into();
                    node_tracking_number += 1;

                    stack_graph.add_edge(comp_unit_node_handle, node_handle, 0);
                    edge_tracking_number += 1;
                    namespace_node_map.insert(namespace_node.symbol.clone(), node_handle);
                }
                2 => {
                    // When there are two nodes, then it must be a class and namespace node.
                    // the list is <class_node, namespace_node>
                    let namespace_node = nodes.last();
                    if namespace_node.is_none() {
                        continue;
                    }
                    let namespace_node = namespace_node.unwrap();
                    let namespace_node_handle =
                        if namespace_node_map.contains_key(&namespace_node.symbol) {
                            *namespace_node_map.get(&namespace_node.symbol).unwrap()
                        } else {
                            let id = stack_graph.new_node_id(file);
                            let symbol = stack_graph.add_symbol(&namespace_node.symbol);
                            let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                            if node_handle.is_none() {
                                continue;
                            }
                            let node_handle = node_handle.unwrap();
                            let syntax_type =
                                stack_graph.add_string(&namespace_node.syntax_type.to_string());
                            let source_info = stack_graph.source_info_mut(node_handle);
                            source_info.syntax_type = syntax_type.into();
                            node_tracking_number += 1;

                            stack_graph.add_edge(comp_unit_node_handle, node_handle, 0);
                            edge_tracking_number += 1;
                            namespace_node_map.insert(namespace_node.symbol.clone(), node_handle);
                            node_handle
                        };
                    let class_node = nodes.first();
                    if class_node.is_none() {
                        continue;
                    }
                    let class_node = class_node.unwrap();
                    let class_node_handle = if type_node_map.contains_key(&class_node.symbol) {
                        *type_node_map.get(&class_node.symbol).unwrap()
                    } else {
                        let id = stack_graph.new_node_id(file);
                        let symbol = stack_graph.add_symbol(&class_node.symbol);
                        let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                        if node_handle.is_none() {
                            continue;
                        }
                        let node_handle = node_handle.unwrap();
                        let syntax_type =
                            stack_graph.add_string(&class_node.syntax_type.to_string());
                        let source_info = stack_graph.source_info_mut(node_handle);
                        source_info.syntax_type = syntax_type.into();
                        node_tracking_number += 1;
                        type_node_map.insert(class_node.symbol.clone(), node_handle);
                        node_handle
                    };
                    stack_graph.add_edge(namespace_node_handle, class_node_handle, 0);
                    stack_graph.add_edge(class_node_handle, namespace_node_handle, 10);
                    edge_tracking_number += 2;
                }
                3 => {
                    // When there are three nodes, then it must be a property(field or method), class and namespace nodes.
                    // the list is <property_node, class_node, namespace_node>
                    let namespace_node = nodes.last();
                    if namespace_node.is_none() {
                        continue;
                    }
                    let namespace_node = namespace_node.unwrap();
                    let namespace_node_handle =
                        if namespace_node_map.contains_key(&namespace_node.symbol) {
                            *namespace_node_map.get(&namespace_node.symbol).unwrap()
                        } else {
                            let id = stack_graph.new_node_id(file);
                            let symbol = stack_graph.add_symbol(&namespace_node.symbol);
                            let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                            if node_handle.is_none() {
                                continue;
                            }
                            node_tracking_number += 1;
                            let node_handle = node_handle.unwrap();
                            let syntax_type =
                                stack_graph.add_string(&namespace_node.syntax_type.to_string());
                            let source_info = stack_graph.source_info_mut(node_handle);
                            source_info.syntax_type = syntax_type.into();

                            stack_graph.add_edge(comp_unit_node_handle, node_handle, 0);
                            edge_tracking_number += 1;
                            namespace_node_map.insert(namespace_node.symbol.clone(), node_handle);
                            node_handle
                        };
                    let class_node = nodes.get(1);
                    if class_node.is_none() {
                        continue;
                    }
                    let class_node = class_node.unwrap();
                    let class_node_handle = if type_node_map.contains_key(&class_node.symbol) {
                        *type_node_map.get(&class_node.symbol).unwrap()
                    } else {
                        let id = stack_graph.new_node_id(file);
                        let symbol = stack_graph.add_symbol(&class_node.symbol);
                        let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                        node_tracking_number += 1;
                        if node_handle.is_none() {
                            continue;
                        }
                        let node_handle = node_handle.unwrap();
                        let syntax_type =
                            stack_graph.add_string(&class_node.syntax_type.to_string());
                        let source_info = stack_graph.source_info_mut(node_handle);
                        source_info.syntax_type = syntax_type.into();
                        type_node_map.insert(class_node.symbol.clone(), node_handle);
                        // If this was not created before, then we need to make sure to add the edges.
                        stack_graph.add_edge(namespace_node_handle, node_handle, 0);
                        stack_graph.add_edge(node_handle, namespace_node_handle, 10);
                        edge_tracking_number += 2;
                        node_handle
                    };
                    let prop_node = nodes.first();
                    if prop_node.is_none() {
                        continue;
                    }
                    let prop_node = prop_node.unwrap();
                    let id = stack_graph.new_node_id(file);
                    let symbol = stack_graph.add_symbol(&prop_node.symbol);
                    let node_handle = stack_graph.add_pop_symbol_node(id, symbol, true);
                    if node_handle.is_none() {
                        continue;
                    }
                    let node_handle = node_handle.unwrap();
                    let syntax_type = stack_graph.add_string(&prop_node.syntax_type.to_string());
                    let source_info = stack_graph.source_info_mut(node_handle);
                    source_info.syntax_type = syntax_type.into();
                    node_tracking_number += 1;
                    stack_graph.add_edge(class_node_handle, node_handle, 0);
                    stack_graph.add_edge(node_handle, class_node_handle, 10);
                    edge_tracking_number += 2;
                }
                _ => {
                    error!("invalid nodes found. continuing with reset of file");
                    continue;
                }
            }
        }

        info!(
            file=?path,
            "created {} graph nodes with {} edges",
            &node_tracking_number,
            &edge_tracking_number
        );
        Ok(())
    }
}

impl DepXMLFileAnalyzer {
    fn handle_member(&self, member_type: &str, name: &str) -> Vec<NodeInfo> {
        match member_type {
            // namespace.
            "N" => {
                let node = NodeInfo {
                    symbol: name.to_string(),
                    syntax_type: SyntaxType::NamespaceDeclaration,
                };
                vec![node]
            }
            // type, field and property
            "T" => {
                if name.is_empty() {
                    return vec![];
                }
                let mut parts = name.split('.');
                let mut nodes: Vec<NodeInfo> = vec![];
                let part = parts.next_back();
                if part.is_none() {
                    return nodes;
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
                nodes
            }
            "F" | "P" => {
                if name.is_empty() {
                    return vec![];
                }
                let mut parts = name.split('.');
                let mut nodes: Vec<NodeInfo> = vec![];
                let part = parts.next_back();
                if part.is_none() {
                    return vec![];
                }
                let field_name = NodeInfo {
                    symbol: part.unwrap().to_string(),
                    syntax_type: SyntaxType::FieldName,
                };
                nodes.push(field_name.clone());
                let part = parts.next_back();
                if part.is_none() {
                    return vec![];
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
                nodes
            }
            "M" => {
                if name.is_empty() {
                    return vec![];
                }
                let mut new_name = name;
                if name.contains('(') {
                    let mut x = name.split('(');
                    let x = x.nth(0);
                    new_name = x.unwrap();
                }
                let mut parts = new_name.split('.');
                let mut nodes: Vec<NodeInfo> = vec![];
                let part = parts.next_back();
                if part.is_none() {
                    return vec![];
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
                        return vec![];
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
                        return vec![];
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
                nodes
            }
            _ => {
                info!("unable to handle: {} -- {}", member_type, name);
                vec![]
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
        let nodes = analyzer.handle_member("N", "System");

        assert_eq!(nodes.len(), 1);

        assert_eq!(nodes[0].symbol, "System");
        assert_eq!(nodes[0].syntax_type, SyntaxType::NamespaceDeclaration);
    }

    #[test]
    fn test_handle_member_namespace_nested() {
        let analyzer = create_analyzer();
        let nodes = analyzer.handle_member("N", "System.Configuration");

        assert_eq!(nodes.len(), 1);

        // Namespace member type returns the full name as-is
        assert_eq!(nodes[0].symbol, "System.Configuration");
        assert_eq!(nodes[0].syntax_type, SyntaxType::NamespaceDeclaration);
    }

    #[test]
    fn test_handle_member_namespace_empty() {
        let analyzer = create_analyzer();
        let nodes = analyzer.handle_member("N", "");

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].symbol, "");
    }

    // Tests for Type (T) member

    #[test]
    fn test_handle_member_type_simple() {
        let analyzer = create_analyzer();
        let nodes = analyzer.handle_member("T", "System.String");

        assert_eq!(nodes.len(), 2);

        // First node should be the class
        assert_eq!(nodes[0].symbol, "String");
        assert_eq!(nodes[0].syntax_type, SyntaxType::ClassDef);

        // Second node should be the namespace
        assert_eq!(nodes[1].symbol, "System");
        assert_eq!(nodes[1].syntax_type, SyntaxType::NamespaceDeclaration);
    }

    #[test]
    fn test_handle_member_type_nested_namespace() {
        let analyzer = create_analyzer();
        let nodes = analyzer.handle_member("T", "System.Configuration.ConfigurationManager");

        assert_eq!(nodes.len(), 2);

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
        let nodes = analyzer.handle_member("T", "String");

        assert_eq!(nodes.len(), 2);

        assert_eq!(nodes[0].symbol, "String");
        assert_eq!(nodes[1].symbol, ""); // Empty namespace
    }

    #[test]
    fn test_handle_member_type_empty_string() {
        let analyzer = create_analyzer();
        let nodes = analyzer.handle_member("T", "");

        // Empty string should return empty vectors
        assert_eq!(nodes.len(), 0);
    }

    // Tests for Field (F) and Property (P) members

    #[test]
    fn test_handle_member_field_simple() {
        let analyzer = create_analyzer();
        let nodes =
            analyzer.handle_member("F", "System.Configuration.ConfigurationManager.AppSettings");

        assert_eq!(nodes.len(), 3);

        // Field node
        assert_eq!(nodes[0].symbol, "AppSettings");
        assert_eq!(nodes[0].syntax_type, SyntaxType::FieldName);

        // Class node
        assert_eq!(nodes[1].symbol, "ConfigurationManager");
        assert_eq!(nodes[1].syntax_type, SyntaxType::ClassDef);

        // Namespace node
        assert_eq!(nodes[2].symbol, "System.Configuration");
        assert_eq!(nodes[2].syntax_type, SyntaxType::NamespaceDeclaration);
    }

    #[test]
    fn test_handle_member_property_same_as_field() {
        let analyzer = create_analyzer();
        let nodes_f = analyzer.handle_member("F", "System.Console.Out");
        let nodes_p = analyzer.handle_member("P", "System.Console.Out");

        // Field and Property should be handled identically
        assert_eq!(nodes_f.len(), nodes_p.len());

        assert_eq!(nodes_f[0].symbol, nodes_p[0].symbol);
        assert_eq!(nodes_f[0].syntax_type, nodes_p[0].syntax_type);
    }

    #[test]
    fn test_handle_member_field_missing_parts() {
        let analyzer = create_analyzer();

        // Only class and field, no namespace
        let nodes = analyzer.handle_member("F", "Console.Out");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[2].symbol, ""); // Empty namespace

        // Only one part - should return empty
        let nodes = analyzer.handle_member("F", "Out");
        assert_eq!(nodes.len(), 0);

        // Empty string
        let nodes = analyzer.handle_member("F", "");
        assert_eq!(nodes.len(), 0);
    }

    // Tests for Method (M) members

    #[test]
    fn test_handle_member_method_simple() {
        let analyzer = create_analyzer();
        let nodes = analyzer.handle_member("M", "System.String.Format");

        assert_eq!(nodes.len(), 3);

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
        let nodes =
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
        let nodes = analyzer.handle_member(
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
        let nodes = analyzer.handle_member("M", "System.String.#ctor");

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
        let nodes = analyzer.handle_member("M", "System.String.#ctor(System.Char[])");

        assert_eq!(nodes.len(), 3);

        // Constructor with parameters - params should be stripped
        assert_eq!(nodes[0].symbol, "String");
        assert_eq!(nodes[1].symbol, "String");
        assert_eq!(nodes[2].symbol, "System");
    }

    #[test]
    fn test_handle_member_method_nested_namespace() {
        let analyzer = create_analyzer();
        let nodes =
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
        let nodes = analyzer.handle_member("M", "String.Format");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[2].symbol, ""); // Empty namespace

        // Only one part - should return empty
        let nodes = analyzer.handle_member("M", "Format");
        assert_eq!(nodes.len(), 0);

        // Empty string
        let nodes = analyzer.handle_member("M", "");
        assert_eq!(nodes.len(), 0);
    }

    #[test]
    fn test_handle_member_method_constructor_missing_class() {
        let analyzer = create_analyzer();
        // Constructor with no class name before it
        let nodes = analyzer.handle_member("M", "#ctor");

        assert_eq!(nodes.len(), 0);
    }

    // Tests for unknown member types

    #[test]
    fn test_handle_member_unknown_type() {
        let analyzer = create_analyzer();
        let nodes = analyzer.handle_member("X", "System.Something");

        assert_eq!(nodes.len(), 0);
    }

    #[test]
    fn test_handle_member_empty_type() {
        let analyzer = create_analyzer();
        let nodes = analyzer.handle_member("", "System.Something");

        assert_eq!(nodes.len(), 0);
    }
    // Integration tests with real-world examples

    #[test]
    fn test_handle_member_real_world_configuration_manager() {
        let analyzer = create_analyzer();
        let nodes =
            analyzer.handle_member("F", "System.Configuration.ConfigurationManager.AppSettings");

        assert_eq!(nodes.len(), 3);

        // Verify the complete graph structure
        let field = &nodes[0];
        let class = &nodes[1];
        let namespace = &nodes[2];

        assert_eq!(field.symbol, "AppSettings");
        assert_eq!(class.symbol, "ConfigurationManager");
        assert_eq!(namespace.symbol, "System.Configuration");
    }

    #[test]
    fn test_handle_member_real_world_linq_method() {
        let analyzer = create_analyzer();
        let nodes = analyzer.handle_member(
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
        let nodes = analyzer.handle_member("T", "System.Collections.Generic.List`1");

        assert_eq!(nodes.len(), 2);
        // Generic type notation is preserved
        assert_eq!(nodes[0].symbol, "List`1");
        assert_eq!(nodes[1].symbol, "System.Collections.Generic");
    }
}
