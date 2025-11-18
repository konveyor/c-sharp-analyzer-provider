use std::{collections::BTreeMap, vec};

use anyhow::{Error, Ok};
use stack_graphs::{
    arena::Handle,
    graph::{Node, StackGraph},
};
use tracing::{debug, trace};

use crate::c_sharp_graph::query::{get_fqdn, Fqdn, GetMatcher, Search, SymbolMatcher, SyntaxType};

pub(crate) struct ClassSymbolsGetter {}

impl GetMatcher for ClassSymbolsGetter {
    type Matcher = ClassSymbols;

    fn get_matcher(
        stack_graphs: &StackGraph,
        definition_root_nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> Result<Self::Matcher, Error>
    where
        Self: std::marker::Sized,
    {
        debug!("getting FieldSymbols matcher");
        ClassSymbols::new(stack_graphs, definition_root_nodes, search)
    }
}

pub(crate) struct ClassSymbols {
    classes: BTreeMap<Fqdn, Handle<Node>>,
}

// Create exposed methods for NamesapceSymbols
impl ClassSymbols {
    pub(crate) fn new(
        graph: &StackGraph,
        nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> anyhow::Result<ClassSymbols, Error> {
        let mut classes: BTreeMap<Fqdn, Handle<Node>> = BTreeMap::new();

        for node_handle in nodes {
            //Get all the edges
            Self::traverse_node(graph, node_handle, search, &mut classes)
        }

        trace!("class nodes found: {:?}", classes);

        Ok(ClassSymbols { classes })
    }
}

impl SymbolMatcher for ClassSymbols {
    fn match_symbol(&self, symbol: String) -> bool {
        self.symbol_in_namespace(symbol)
    }
    fn match_fqdn(&self, fqdn: &Fqdn) -> bool {
        if (fqdn.method.is_some() || fqdn.field.is_some()) && fqdn.class.is_some() {
            return self.classes.contains_key(&Fqdn {
                namespace: fqdn.namespace.clone(),
                class: fqdn.class.clone(),
                method: None,
                field: None,
            });
        }
        self.classes.contains_key(fqdn)
    }
}

// Private methods for NamespaceSymbols
impl ClassSymbols {
    fn traverse_node(
        graph: &StackGraph,
        node: Handle<Node>,
        search: &Search,
        classes: &mut BTreeMap<Fqdn, Handle<Node>>,
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
            trace!("got node: {:?}, symbol: {} matching", edge.sink, symbol,);
            match graph.source_info(edge.sink) {
                None => continue,
                Some(source_info) => match source_info.syntax_type.into_option() {
                    None => continue,
                    Some(syntax_type) => {
                        if let SyntaxType::ClassDef = SyntaxType::get(&graph[syntax_type]) {
                            let fqdn_name = get_fqdn(edge.sink, graph)
                                .expect("We should always get a FQDN for methods");
                            classes.insert(fqdn_name, edge.sink);
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
            Self::traverse_node(graph, child_edge, search, classes);
        }
    }

    // Symbol here must be of <thing>.<method_name>.
    // <thing> may be a class or a variable.
    // if a variable, we may have to enhance this method
    // to get the actual "class" of the variable.
    // TODO: Consider scoped things for this(??)
    // TODO: Consider a edge from the var to the class symbol
    fn symbol_in_namespace(&self, symbol: String) -> bool {
        self.classes.keys().any(|fqdn| {
            let class = fqdn.class.clone().unwrap_or("".to_string());
            class == symbol
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::c_sharp_graph::query::Search;

    /// Helper to build a simple mock graph with classes
    fn build_mock_graph_with_classes() -> (StackGraph, Vec<Handle<Node>>) {
        let mut graph = StackGraph::new();
        let file = graph.add_file("test.cs").unwrap();

        // Create root node
        let root_id = graph.new_node_id(file);
        let root_symbol = graph.add_symbol("root");
        let root = graph.add_pop_symbol_node(root_id, root_symbol, true).unwrap();

        // Create namespace: System
        let ns_id = graph.new_node_id(file);
        let ns_symbol = graph.add_symbol("System");
        let ns_node = graph.add_pop_symbol_node(ns_id, ns_symbol, true).unwrap();
        let ns_syntax = graph.add_string("namespace_declaration");
        graph.source_info_mut(ns_node).syntax_type = ns_syntax.into();

        // Create class: String
        let class1_id = graph.new_node_id(file);
        let class1_symbol = graph.add_symbol("String");
        let class1_node = graph.add_pop_symbol_node(class1_id, class1_symbol, true).unwrap();
        let class_syntax = graph.add_string("class_def");
        graph.source_info_mut(class1_node).syntax_type = class_syntax.into();

        // Create class: StringBuilder
        let class2_id = graph.new_node_id(file);
        let class2_symbol = graph.add_symbol("StringBuilder");
        let class2_node = graph.add_pop_symbol_node(class2_id, class2_symbol, true).unwrap();
        graph.source_info_mut(class2_node).syntax_type = class_syntax.into();

        // Add edges: root -> namespace (precedence 0)
        graph.add_edge(root, ns_node, 0);

        // Add edges: namespace -> class1 (precedence 0)
        graph.add_edge(ns_node, class1_node, 0);
        // Add FQDN edge: class1 -> namespace (precedence 10)
        graph.add_edge(class1_node, ns_node, 10);

        // Add edges: namespace -> class2 (precedence 0)
        graph.add_edge(ns_node, class2_node, 0);
        // Add FQDN edge: class2 -> namespace (precedence 10)
        graph.add_edge(class2_node, ns_node, 10);

        (graph, vec![root])
    }

    #[test]
    fn test_class_symbols_new() {
        let (graph, roots) = build_mock_graph_with_classes();
        let search = Search::create_search("*".to_string()).unwrap();

        let result = ClassSymbols::new(&graph, roots, &search);
        assert!(result.is_ok());

        let class_symbols = result.unwrap();
        assert_eq!(class_symbols.classes.len(), 2);
    }

    #[test]
    fn test_class_symbols_match_symbol() {
        let (graph, roots) = build_mock_graph_with_classes();
        let search = Search::create_search("*".to_string()).unwrap();
        let class_symbols = ClassSymbols::new(&graph, roots, &search).unwrap();

        assert!(class_symbols.match_symbol("String".to_string()));
        assert!(class_symbols.match_symbol("StringBuilder".to_string()));
        assert!(!class_symbols.match_symbol("NonExistent".to_string()));
    }

    #[test]
    fn test_class_symbols_match_fqdn_exact() {
        let (graph, roots) = build_mock_graph_with_classes();
        let search = Search::create_search("*".to_string()).unwrap();
        let class_symbols = ClassSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System".to_string()),
            class: Some("String".to_string()),
            method: None,
            field: None,
        };

        assert!(class_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_class_symbols_match_fqdn_with_method() {
        let (graph, roots) = build_mock_graph_with_classes();
        let search = Search::create_search("*".to_string()).unwrap();
        let class_symbols = ClassSymbols::new(&graph, roots, &search).unwrap();

        // When FQDN has method, should still match the class part
        let fqdn = Fqdn {
            namespace: Some("System".to_string()),
            class: Some("String".to_string()),
            method: Some("Format".to_string()),
            field: None,
        };

        assert!(class_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_class_symbols_match_fqdn_with_field() {
        let (graph, roots) = build_mock_graph_with_classes();
        let search = Search::create_search("*".to_string()).unwrap();
        let class_symbols = ClassSymbols::new(&graph, roots, &search).unwrap();

        // When FQDN has field, should still match the class part
        let fqdn = Fqdn {
            namespace: Some("System".to_string()),
            class: Some("String".to_string()),
            field: Some("Empty".to_string()),
            method: None,
        };

        assert!(class_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_class_symbols_no_match_fqdn() {
        let (graph, roots) = build_mock_graph_with_classes();
        let search = Search::create_search("*".to_string()).unwrap();
        let class_symbols = ClassSymbols::new(&graph, roots, &search).unwrap();

        let fqdn = Fqdn {
            namespace: Some("System".to_string()),
            class: Some("NonExistent".to_string()),
            method: None,
            field: None,
        };

        assert!(!class_symbols.match_fqdn(&fqdn));
    }

    #[test]
    fn test_class_symbols_with_search_filter() {
        let (graph, roots) = build_mock_graph_with_classes();
        // Only match "String" not "StringBuilder"
        let search = Search::create_search("String".to_string()).unwrap();

        let class_symbols = ClassSymbols::new(&graph, roots, &search).unwrap();

        // Should only have String, not StringBuilder
        assert_eq!(class_symbols.classes.len(), 1);
        assert!(class_symbols.match_symbol("String".to_string()));
        assert!(!class_symbols.match_symbol("StringBuilder".to_string()));
    }
}
