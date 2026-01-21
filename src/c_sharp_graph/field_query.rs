use std::{collections::BTreeMap, vec};

use anyhow::{anyhow, Error, Ok};
use stack_graphs::{
    arena::Handle,
    graph::{Node, StackGraph},
};
use tracing::{debug, trace};

use crate::c_sharp_graph::{
    namespace_query::NotFoundError,
    query::{get_fqdn, Fqdn, GetMatcher, Search, SymbolMatcher, SyntaxType},
};

pub(crate) struct FieldSymbolsGetter {}

impl GetMatcher for FieldSymbolsGetter {
    type Matcher = FieldSymbols;

    fn get_matcher(
        stack_graphs: &StackGraph,
        definition_root_nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> Result<Self::Matcher, Error>
    where
        Self: std::marker::Sized,
    {
        debug!("getting FieldSymbols matcher");
        FieldSymbols::new(stack_graphs, definition_root_nodes, search)
    }
}

#[derive(Debug)]
pub(crate) struct FieldSymbols {
    fields: BTreeMap<Fqdn, Handle<Node>>,
}

// Create exposed methods for NamesapceSymbols
impl FieldSymbols {
    pub(crate) fn new(
        graph: &StackGraph,
        nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> anyhow::Result<FieldSymbols, Error> {
        let mut fields: BTreeMap<Fqdn, Handle<Node>> = BTreeMap::new();

        for node_handle in nodes {
            //Get all the edges
            Self::traverse_node(graph, node_handle, search, &mut fields)
        }
        if fields.is_empty() {
            return Err(anyhow!(NotFoundError {}));
        }
        trace!("field nodes found: {:?}", fields);

        Ok(FieldSymbols { fields })
    }
}

impl SymbolMatcher for FieldSymbols {
    fn match_symbol(&self, symbol: String) -> bool {
        self.symbol_in_namespace(symbol)
    }
    fn match_fqdn(&self, fqdn: &Fqdn) -> bool {
        self.fields.contains_key(fqdn)
    }
}

// Private methods for NamespaceSymbols
impl FieldSymbols {
    fn traverse_node(
        graph: &StackGraph,
        node: Handle<Node>,
        search: &Search,
        fields: &mut BTreeMap<Fqdn, Handle<Node>>,
    ) {
        // Collect and sort all edges first to ensure deterministic iteration
        let mut edges: Vec<_> = graph.outgoing_edges(node).collect();
        edges.sort_by_key(|e| e.sink);

        let mut child_edges: Vec<Handle<Node>> = vec![];
        for edge in edges {
            if edge.precedence == 10 {
                continue;
            }
            child_edges.push(edge.sink);
            let child_node = &graph[edge.sink];
            let symbol = match child_node.symbol() {
                None => continue,
                Some(symbol) => &graph[symbol],
            };
            if !search.match_symbol(symbol) {
                continue;
            }
            match graph.source_info(edge.sink) {
                None => continue,
                Some(source_info) => match source_info.syntax_type.into_option() {
                    None => continue,
                    Some(syntax_type) => {
                        if let SyntaxType::FieldName = SyntaxType::get(&graph[syntax_type]) {
                            if let Some(fqdn_name) = get_fqdn(edge.sink, graph) {
                                if search.match_namespace(&fqdn_name.get_full_symbol()) {
                                    fields.insert(fqdn_name, edge.sink);
                                }
                            }
                        } else {
                            trace!(
                                "got node: {:?}, symbol: {} not matching syntax_type: {}",
                                edge.sink,
                                symbol,
                                &graph[syntax_type]
                            );
                        }
                    }
                },
            }
        }
        // Recursively traverse child edges (already in sorted order)
        for child_edge in child_edges {
            Self::traverse_node(graph, child_edge, search, fields);
        }
    }

    // Symbol here must be of <thing>.<method_name>.
    // <thing> may be a class or a variable.
    // if a variable, we may have to enhance this method
    // to get the actual "class" of the variable.
    // TODO: Consider scoped things for this(??)
    // TODO: Consider a edge from the var to the class symbol
    fn symbol_in_namespace(&self, symbol: String) -> bool {
        let parts: Vec<&str> = symbol.split(".").collect();
        if parts.len() != 2 {
            return false;
        }
        let field_part = parts
            .last()
            .expect("unable to get method part for symbol")
            .to_string();
        let class_part = parts
            .first()
            .expect("unable to get class part for symbol")
            .to_string();
        self.fields.keys().any(|fqdn| {
            let field = fqdn.field.clone().unwrap_or("".to_string());
            let class = fqdn.class.clone().unwrap_or("".to_string());
            field == field_part && class == class_part
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::c_sharp_graph::query::Search;

    /// Helper to build a mock graph with fields
    fn build_mock_graph_with_fields() -> (StackGraph, Vec<Handle<Node>>) {
        let mut graph = StackGraph::new();
        let file = graph.add_file("test.cs").unwrap();

        // Create root node
        let root_id = graph.new_node_id(file);
        let root_symbol = graph.add_symbol("root");
        let root = graph
            .add_pop_symbol_node(root_id, root_symbol, true)
            .unwrap();

        // Create namespace: System
        let ns_id = graph.new_node_id(file);
        let ns_symbol = graph.add_symbol("System");
        let ns_node = graph.add_pop_symbol_node(ns_id, ns_symbol, true).unwrap();
        let ns_syntax = graph.add_string("namespace_declaration");
        graph.source_info_mut(ns_node).syntax_type = ns_syntax.into();

        // Create class: Console
        let class_id = graph.new_node_id(file);
        let class_symbol = graph.add_symbol("Console");
        let class_node = graph
            .add_pop_symbol_node(class_id, class_symbol, true)
            .unwrap();
        let class_syntax = graph.add_string("class_def");
        graph.source_info_mut(class_node).syntax_type = class_syntax.into();

        // Create field: Out
        let field1_id = graph.new_node_id(file);
        let field1_symbol = graph.add_symbol("Out");
        let field1_node = graph
            .add_pop_symbol_node(field1_id, field1_symbol, true)
            .unwrap();
        let field_syntax = graph.add_string("field_name");
        graph.source_info_mut(field1_node).syntax_type = field_syntax.into();

        // Create field: Error
        let field2_id = graph.new_node_id(file);
        let field2_symbol = graph.add_symbol("Error");
        let field2_node = graph
            .add_pop_symbol_node(field2_id, field2_symbol, true)
            .unwrap();
        graph.source_info_mut(field2_node).syntax_type = field_syntax.into();

        // Build edge structure
        graph.add_edge(root, ns_node, 0);
        graph.add_edge(ns_node, class_node, 0);
        graph.add_edge(class_node, ns_node, 10); // FQDN edge

        graph.add_edge(class_node, field1_node, 0);
        graph.add_edge(field1_node, class_node, 10); // FQDN edge

        graph.add_edge(class_node, field2_node, 0);
        graph.add_edge(field2_node, class_node, 10); // FQDN edge

        (graph, vec![root])
    }

    #[test]
    fn test_field_symbols_new() {
        let (graph, roots) = build_mock_graph_with_fields();
        let search = Search::create_search("*".to_string()).unwrap();

        let result = FieldSymbols::new(&graph, roots, &search);
        assert!(result.is_ok());

        let field_symbols = result.unwrap();
        assert_eq!(field_symbols.fields.len(), 2);
    }

    #[test]
    fn test_field_symbols_match_symbol_valid_format() {
        let (graph, roots) = build_mock_graph_with_fields();
        let search = Search::create_search("*".to_string()).unwrap();
        let field_symbols = FieldSymbols::new(&graph, roots, &search).unwrap();

        // Valid format: Class.Field
        assert!(field_symbols.match_symbol("Console.Out".to_string()));
        assert!(field_symbols.match_symbol("Console.Error".to_string()));
    }

    #[test]
    fn test_field_symbols_match_symbol_invalid_format() {
        let (graph, roots) = build_mock_graph_with_fields();
        let search = Search::create_search("*".to_string()).unwrap();
        let field_symbols = FieldSymbols::new(&graph, roots, &search).unwrap();

        // Invalid formats
        assert!(!field_symbols.match_symbol("Out".to_string())); // No class
        assert!(!field_symbols.match_symbol("System.Console.Out".to_string())); // Too many parts
        assert!(!field_symbols.match_symbol("Console.NonExistent".to_string())); // Wrong field
        assert!(!field_symbols.match_symbol("WrongClass.Out".to_string())); // Wrong class
    }

    #[test]
    fn test_field_symbols_match_fqdn() {
        let (graph, roots) = build_mock_graph_with_fields();
        let search = Search::create_search("*".to_string()).unwrap();
        let field_symbols = FieldSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System".to_string()),
            class: Some("Console".to_string()),
            field: Some("Out".to_string()),
            method: None,
        };

        assert!(field_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_field_symbols_no_match_fqdn() {
        let (graph, roots) = build_mock_graph_with_fields();
        let search = Search::create_search("*".to_string()).unwrap();
        let field_symbols = FieldSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System".to_string()),
            class: Some("Console".to_string()),
            field: Some("NonExistent".to_string()),
            method: None,
        };

        assert!(!field_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_field_symbols_with_search_filter() {
        let (graph, roots) = build_mock_graph_with_fields();
        // Only match "Out" not "Error"
        let search = Search::create_search("*.Out".to_string()).unwrap();

        let field_symbols = FieldSymbols::new(&graph, roots, &search).unwrap();

        // Should only have Out
        assert_eq!(field_symbols.fields.len(), 1);
        assert!(field_symbols.match_symbol("Console.Out".to_string()));
        assert!(!field_symbols.match_symbol("Console.Error".to_string()));
    }
}
