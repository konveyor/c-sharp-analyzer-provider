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

pub(crate) struct MethodSymbolsGetter {}

impl GetMatcher for MethodSymbolsGetter {
    type Matcher = MethodSymbols;

    fn get_matcher(
        stack_graphs: &StackGraph,
        definition_root_nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> Result<Self::Matcher, Error>
    where
        Self: std::marker::Sized,
    {
        debug!("getting MethodSymbols matcher");
        MethodSymbols::new(stack_graphs, definition_root_nodes, search)
    }
}

#[derive(Debug)]
pub(crate) struct MethodSymbols {
    methods: BTreeMap<Fqdn, Handle<Node>>,
}

// Create exposed methods for NamesapceSymbols
impl MethodSymbols {
    pub(crate) fn new(
        graph: &StackGraph,
        nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> anyhow::Result<MethodSymbols, Error> {
        let mut methods: BTreeMap<Fqdn, Handle<Node>> = BTreeMap::new();

        for node_handle in nodes {
            //Get all the edges
            Self::traverse_node(graph, node_handle, search, &mut methods)
        }

        if methods.is_empty() {
            return Err(anyhow!(NotFoundError {}));
        }
        trace!("method nodes found: {:?}", methods);

        Ok(MethodSymbols { methods })
    }
}

impl SymbolMatcher for MethodSymbols {
    fn match_symbol(&self, symbol: String) -> bool {
        self.symbol_in_namespace(symbol)
    }
    fn match_fqdn(&self, fqdn: &Fqdn) -> bool {
        self.methods.contains_key(fqdn)
    }
}

// Private methods for NamespaceSymbols
impl MethodSymbols {
    fn traverse_node(
        graph: &StackGraph,
        node: Handle<Node>,
        search: &Search,
        methods: &mut BTreeMap<Fqdn, Handle<Node>>,
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
                        if let SyntaxType::MethodName = SyntaxType::get(&graph[syntax_type]) {
                            let fqdn_name = get_fqdn(edge.sink, graph)
                                .expect("We should always get a FQDN for methods");
                            methods.insert(fqdn_name, edge.sink);
                        }
                    }
                },
            }
        }
        // Recursively traverse child edges (already in sorted order)
        for child_edge in child_edges {
            Self::traverse_node(graph, child_edge, search, methods);
        }
    }

    // Symbol here must be of <thing>.<method_name>.
    // <thing> may be a class or a variable.
    // if a variable, we may have to enhance this method
    // to get the actual "class" of the variable.
    // TODO: Consider scoped things for this(??)
    // TODO: Consider a edge from the var to the class symbol
    fn symbol_in_namespace(&self, symbol: String) -> bool {
        trace!("checking symbol: {}", symbol);
        let parts: Vec<&str> = symbol.split(".").collect();
        if parts.len() != 2 {
            return false;
        }
        let method_part = parts
            .last()
            .expect("unable to get method part for symbol")
            .to_string();
        let class_part = parts
            .first()
            .expect("unable to get class part for symbol")
            .to_string();
        self.methods.keys().any(|fqdn| {
            let method = fqdn.method.clone().unwrap_or("".to_string());
            let class = fqdn.class.clone().unwrap_or("".to_string());
            method == method_part && class == class_part
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::c_sharp_graph::query::Search;

    /// Helper to build a mock graph with methods
    fn build_mock_graph_with_methods() -> (StackGraph, Vec<Handle<Node>>) {
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

        // Create class: String
        let class_id = graph.new_node_id(file);
        let class_symbol = graph.add_symbol("String");
        let class_node = graph
            .add_pop_symbol_node(class_id, class_symbol, true)
            .unwrap();
        let class_syntax = graph.add_string("class_def");
        graph.source_info_mut(class_node).syntax_type = class_syntax.into();

        // Create method: Format
        let method1_id = graph.new_node_id(file);
        let method1_symbol = graph.add_symbol("Format");
        let method1_node = graph
            .add_pop_symbol_node(method1_id, method1_symbol, true)
            .unwrap();
        let method_syntax = graph.add_string("method_name");
        graph.source_info_mut(method1_node).syntax_type = method_syntax.into();

        // Create method: Concat
        let method2_id = graph.new_node_id(file);
        let method2_symbol = graph.add_symbol("Concat");
        let method2_node = graph
            .add_pop_symbol_node(method2_id, method2_symbol, true)
            .unwrap();
        graph.source_info_mut(method2_node).syntax_type = method_syntax.into();

        // Build edge structure
        graph.add_edge(root, ns_node, 0);
        graph.add_edge(ns_node, class_node, 0);
        graph.add_edge(class_node, ns_node, 10); // FQDN edge

        graph.add_edge(class_node, method1_node, 0);
        graph.add_edge(method1_node, class_node, 10); // FQDN edge

        graph.add_edge(class_node, method2_node, 0);
        graph.add_edge(method2_node, class_node, 10); // FQDN edge

        (graph, vec![root])
    }

    #[test]
    fn test_method_symbols_new() {
        let (graph, roots) = build_mock_graph_with_methods();
        let search = Search::create_search("*".to_string()).unwrap();

        let result = MethodSymbols::new(&graph, roots, &search);
        assert!(result.is_ok());

        let method_symbols = result.unwrap();
        assert_eq!(method_symbols.methods.len(), 2);
    }

    #[test]
    fn test_method_symbols_match_symbol_valid_format() {
        let (graph, roots) = build_mock_graph_with_methods();
        let search = Search::create_search("*".to_string()).unwrap();
        let method_symbols = MethodSymbols::new(&graph, roots, &search).unwrap();

        // Valid format: Class.Method
        assert!(method_symbols.match_symbol("String.Format".to_string()));
        assert!(method_symbols.match_symbol("String.Concat".to_string()));
    }

    #[test]
    fn test_method_symbols_match_symbol_invalid_format() {
        let (graph, roots) = build_mock_graph_with_methods();
        let search = Search::create_search("*".to_string()).unwrap();
        let method_symbols = MethodSymbols::new(&graph, roots, &search).unwrap();

        // Invalid formats
        assert!(!method_symbols.match_symbol("Format".to_string())); // No class
        assert!(!method_symbols.match_symbol("System.String.Format".to_string())); // Too many parts
        assert!(!method_symbols.match_symbol("String.NonExistent".to_string())); // Wrong method
        assert!(!method_symbols.match_symbol("WrongClass.Format".to_string())); // Wrong class
    }

    #[test]
    fn test_method_symbols_match_fqdn() {
        let (graph, roots) = build_mock_graph_with_methods();
        let search = Search::create_search("*".to_string()).unwrap();
        let method_symbols = MethodSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System".to_string()),
            class: Some("String".to_string()),
            method: Some("Format".to_string()),
            field: None,
        };

        assert!(method_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_method_symbols_no_match_fqdn() {
        let (graph, roots) = build_mock_graph_with_methods();
        let search = Search::create_search("*".to_string()).unwrap();
        let method_symbols = MethodSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System".to_string()),
            class: Some("String".to_string()),
            method: Some("NonExistent".to_string()),
            field: None,
        };

        assert!(!method_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_method_symbols_with_search_filter() {
        let (graph, roots) = build_mock_graph_with_methods();
        // Only match "Format" not "Concat"
        let search = Search::create_search("Format".to_string()).unwrap();

        let method_symbols = MethodSymbols::new(&graph, roots, &search).unwrap();

        // Should only have Format
        assert_eq!(method_symbols.methods.len(), 1);
        assert!(method_symbols.match_symbol("String.Format".to_string()));
        assert!(!method_symbols.match_symbol("String.Concat".to_string()));
    }
}
