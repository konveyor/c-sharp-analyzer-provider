use core::fmt;
use std::vec;

use anyhow::{anyhow, Error, Ok};
use stack_graphs::{
    arena::Handle,
    graph::{Node, StackGraph},
};
use tracing::{error, info};

use crate::c_sharp_graph::{
    class_query::ClassSymbols,
    field_query::FieldSymbols,
    method_query::MethodSymbols,
    query::{get_fqdn, Fqdn, GetMatcher, Search, SymbolMatcher, SyntaxType},
};

#[derive(Debug, Clone)]
pub struct NamespaceFQDNNotFoundError;

impl fmt::Display for NamespaceFQDNNotFoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unable to find FQDN for namespace")
    }
}

impl std::error::Error for NamespaceFQDNNotFoundError {}

pub(crate) struct NamespaceSymbolsGetter {}

impl GetMatcher for NamespaceSymbolsGetter {
    type Matcher = NamespaceSymbols;

    fn get_matcher(
        stack_graphs: &StackGraph,
        definition_root_nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> Result<Self::Matcher, Error>
    where
        Self: std::marker::Sized,
    {
        NamespaceSymbols::new(stack_graphs, definition_root_nodes, search)
    }
}

#[derive(Debug)]
pub(crate) struct NamespaceSymbols {
    classes: ClassSymbols,
    fields: FieldSymbols,
    methods: MethodSymbols,
    namespace: Vec<Fqdn>,
}

// Create exposed methods for NamesapceSymbols
impl NamespaceSymbols {
    pub(crate) fn new(
        graph: &StackGraph,
        nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> anyhow::Result<NamespaceSymbols, Error> {
        // TODO: Handle borrow in new function
        let class_symbol = ClassSymbols::new(graph, nodes.clone(), search)?;
        let field_symbol = FieldSymbols::new(graph, nodes.clone(), search)?;
        let method_symbols = MethodSymbols::new(graph, nodes.clone(), search)?;

        let mut results: Vec<Fqdn> = vec![];
        for node in nodes {
            Self::traverse_node(graph, node, search, &mut results);
        }

        if results.is_empty() {
            return Err(anyhow!(NamespaceFQDNNotFoundError {}));
        }

        Ok(NamespaceSymbols {
            classes: class_symbol,
            fields: field_symbol,
            methods: method_symbols,
            namespace: results,
        })
    }
}

impl SymbolMatcher for NamespaceSymbols {
    fn match_symbol(&self, symbol: String) -> bool {
        if self
            .namespace
            .iter()
            .any(|f| f.namespace.as_ref().is_some_and(|n| n == &symbol))
        {
            return true;
        }
        self.classes.match_symbol(symbol.clone())
            || self.fields.match_symbol(symbol.clone())
            || self.methods.match_symbol(symbol.clone())
    }

    fn match_fqdn(&self, fqdn: &Fqdn) -> bool {
        self.classes.match_fqdn(fqdn)
            || self.fields.match_fqdn(fqdn)
            || self.methods.match_fqdn(fqdn)
    }
}

// Private methods for NamespaceSymbols
impl NamespaceSymbols {
    fn traverse_node(
        db: &StackGraph,
        node: Handle<Node>,
        search: &Search,
        results: &mut Vec<Fqdn>,
    ) {
        // We need to handle the first node.
        match db.source_info(node) {
            None => (),
            Some(source_info) => match source_info.syntax_type.into_option() {
                None => (),
                Some(syntax_type) => {
                    if let SyntaxType::NamespaceDeclaration = SyntaxType::get(&db[syntax_type]) {
                        if let Some(fqdn) = get_fqdn(node, db) {
                            if fqdn.namespace.is_some()
                                && search.match_namespace(&fqdn.namespace.clone().unwrap())
                            {
                                results.push(fqdn);
                            }
                        }
                    }
                }
            },
        }
        let mut child_edges: Vec<Handle<Node>> = vec![];
        for edge in db.outgoing_edges(node) {
            if edge.precedence == 10 {
                continue;
            }
            child_edges.push(edge.sink);
        }
        // Sort child_edges to ensure deterministic traversal order
        child_edges.sort();
        for child_edge in child_edges {
            Self::traverse_node(db, child_edge, search, results)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::c_sharp_graph::query::Search;

    /// Helper to build a comprehensive mock graph with namespace, class, method, and field
    fn build_mock_namespace_graph() -> (StackGraph, Vec<Handle<Node>>) {
        let mut graph = StackGraph::new();
        let file = graph.add_file("test.cs").unwrap();

        // Create root node
        let root_id = graph.new_node_id(file);
        let root_symbol = graph.add_symbol("root");
        let root = graph
            .add_pop_symbol_node(root_id, root_symbol, true)
            .unwrap();

        // Create namespace: System.Configuration
        let ns_id = graph.new_node_id(file);
        let ns_symbol = graph.add_symbol("System.Configuration");
        let ns_node = graph.add_pop_symbol_node(ns_id, ns_symbol, true).unwrap();
        let ns_syntax = graph.add_string("namespace_declaration");
        graph.source_info_mut(ns_node).syntax_type = ns_syntax.into();

        // Create namespace: System.Configuration.Web
        let ns_2_id = graph.new_node_id(file);
        let ns_2_symbol = graph.add_symbol("System.Configuration.Web");
        let ns_2_node = graph
            .add_pop_symbol_node(ns_2_id, ns_2_symbol, true)
            .unwrap();
        graph.source_info_mut(ns_2_node).syntax_type = ns_syntax.into();

        // Create namespace: System.Configuration.File
        let ns_3_id = graph.new_node_id(file);
        let ns_3_symbol = graph.add_symbol("System.Configuration.File");
        let ns_3_node = graph
            .add_pop_symbol_node(ns_3_id, ns_3_symbol, true)
            .unwrap();
        graph.source_info_mut(ns_3_node).syntax_type = ns_syntax.into();

        // Create class: ConfigurationManager
        let class_id = graph.new_node_id(file);
        let class_symbol = graph.add_symbol("ConfigurationManager");
        let class_node = graph
            .add_pop_symbol_node(class_id, class_symbol, true)
            .unwrap();
        let class_syntax = graph.add_string("class_def");
        graph.source_info_mut(class_node).syntax_type = class_syntax.into();

        // Create method: GetSection
        let method_id = graph.new_node_id(file);
        let method_symbol = graph.add_symbol("GetSection");
        let method_node = graph
            .add_pop_symbol_node(method_id, method_symbol, true)
            .unwrap();
        let method_syntax = graph.add_string("method_name");
        graph.source_info_mut(method_node).syntax_type = method_syntax.into();

        // Create field: AppSettings
        let field_id = graph.new_node_id(file);
        let field_symbol = graph.add_symbol("AppSettings");
        let field_node = graph
            .add_pop_symbol_node(field_id, field_symbol, true)
            .unwrap();
        let field_syntax = graph.add_string("field_name");
        graph.source_info_mut(field_node).syntax_type = field_syntax.into();

        // Build edge structure
        graph.add_edge(root, ns_node, 0);
        graph.add_edge(root, ns_2_node, 0);
        graph.add_edge(root, ns_3_node, 0);
        graph.add_edge(ns_node, class_node, 0);
        graph.add_edge(class_node, ns_node, 10); // FQDN edge

        graph.add_edge(class_node, method_node, 0);
        graph.add_edge(method_node, class_node, 10); // FQDN edge

        graph.add_edge(class_node, field_node, 0);
        graph.add_edge(field_node, class_node, 10); // FQDN edge

        (graph, vec![root])
    }

    #[test]
    fn test_namespace_symbols_new() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();

        let result = NamespaceSymbols::new(&graph, roots, &search);
        assert!(result.is_ok());

        let ns_symbols = result.unwrap();
        assert_eq!(ns_symbols.namespace.len(), 3)
    }

    #[test]
    fn test_namespace_intra_regex() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*.Configuration.*".to_string()).unwrap();

        let result = NamespaceSymbols::new(&graph, roots, &search);
        assert!(result.is_ok());

        let ns_symbols = result.unwrap();
        assert_eq!(ns_symbols.namespace.len(), 3)
    }

    #[test]
    fn test_namespace_symbols_match_symbol_namespace() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        // Should match the namespace itself
        assert!(ns_symbols.match_symbol("System.Configuration".to_string()));
    }

    #[test]
    fn test_namespace_symbols_match_symbol_class() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        // Should match class within namespace
        assert!(ns_symbols.match_symbol("ConfigurationManager".to_string()));
    }

    #[test]
    fn test_namespace_symbols_match_symbol_method() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        // Should match method (Class.Method format)
        assert!(ns_symbols.match_symbol("ConfigurationManager.GetSection".to_string()));
    }

    #[test]
    fn test_namespace_symbols_match_symbol_field() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        // Should match field (Class.Field format)
        assert!(ns_symbols.match_symbol("ConfigurationManager.AppSettings".to_string()));
    }

    #[test]
    fn test_namespace_symbols_regex_search_match() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("System.Configuration.*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        assert!(ns_symbols.match_symbol("System.Configuration.Web".to_string()));
        assert!(ns_symbols.match_symbol("System.Configuration.File".to_string()));
        assert!(ns_symbols.match_symbol("System.Configuration".to_string()));
    }

    #[test]
    fn test_namespace_exact_match() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("System.Configuration".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        // Should match field (Class.Field format)
        assert!(!ns_symbols.match_symbol("System.Configuration.Web".to_string()));
        assert!(!ns_symbols.match_symbol("System.Configuration.File".to_string()));
        assert!(ns_symbols.match_symbol("System.Configuration".to_string()));
    }

    #[test]
    fn test_namespace_symbols_no_match_symbol() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        assert!(!ns_symbols.match_symbol("NonExistent".to_string()));
        assert!(!ns_symbols.match_symbol("Other.Namespace".to_string()));
    }

    #[test]
    fn test_namespace_symbols_match_fqdn_class() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System.Configuration".to_string()),
            class: Some("ConfigurationManager".to_string()),
            method: None,
            field: None,
        };

        assert!(ns_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_namespace_symbols_match_fqdn_method() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System.Configuration".to_string()),
            class: Some("ConfigurationManager".to_string()),
            method: Some("GetSection".to_string()),
            field: None,
        };

        assert!(ns_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_namespace_symbols_match_fqdn_field() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System.Configuration".to_string()),
            class: Some("ConfigurationManager".to_string()),
            field: Some("AppSettings".to_string()),
            method: None,
        };

        assert!(ns_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_namespace_symbols_no_match_fqdn() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("*".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System.Configuration".to_string()),
            class: Some("NonExistent".to_string()),
            method: None,
            field: None,
        };

        assert!(!ns_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_namespace_symbols_error_when_no_namespace_found() {
        let mut graph = StackGraph::new();
        let file = graph.add_file("test.cs").unwrap();

        // Create root with no namespace children
        let root_id = graph.new_node_id(file);
        let root_symbol = graph.add_symbol("root");
        let root = graph
            .add_pop_symbol_node(root_id, root_symbol, true)
            .unwrap();

        let search = Search::create_search("*".to_string()).unwrap();
        let result = NamespaceSymbols::new(&graph, vec![root], &search);

        // Should return error when no namespace is found
        assert!(result.is_err());
    }

    #[test]
    fn test_non_ending_regex() {
        let (graph, roots) = build_mock_namespace_graph();
        let search = Search::create_search("System.*.Web".to_string()).unwrap();
        let ns_symbols = NamespaceSymbols::new(&graph, roots, &search).unwrap();
        // Should return error when no namespace is found
        assert!(ns_symbols.match_symbol("System.Configuration.Web".to_string()));
    }
}
