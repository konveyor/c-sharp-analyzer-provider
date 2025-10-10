use std::{collections::HashMap, vec};

use anyhow::{Error, Ok};
use stack_graphs::{
    arena::Handle,
    graph::{Node, StackGraph},
};
use tracing::{debug, trace};

use crate::c_sharp_graph::query::{get_fqdn, Fqdn, GetMatcher, Search, SymbolMatcher, SyntaxType};

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

pub(crate) struct FieldSymbols {
    fields: HashMap<Fqdn, Handle<Node>>,
}

// Create exposed methods for NamesapceSymbols
impl FieldSymbols {
    pub(crate) fn new(
        graph: &StackGraph,
        nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> anyhow::Result<FieldSymbols, Error> {
        let mut fields: HashMap<Fqdn, Handle<Node>> = HashMap::new();

        debug!("searching fields in {} root_nodes", nodes.len());
        for node_handle in nodes {
            //Get all the edges
            Self::traverse_node(graph, node_handle, search, &mut fields)
        }

        debug!("field nodes found: {:?}", fields);

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
        fields: &mut HashMap<Fqdn, Handle<Node>>,
    ) {
        let mut child_edges: Vec<Handle<Node>> = vec![];
        for edge in graph.outgoing_edges(node) {
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
                        if let SyntaxType::FieldName = SyntaxType::get(&graph[syntax_type]) {
                            let fqdn_name = get_fqdn(edge.sink, graph)
                                .expect("We should always get a FQDN for methods");
                            fields.insert(fqdn_name, node);
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
            let field = fqdn.method.clone().unwrap_or("".to_string());
            let class = fqdn.class.clone().unwrap_or("".to_string());
            if field == field_part {
                trace!(
                    "here: {:?}.{:?} -- {:?} ",
                    class_part.clone(),
                    field_part.clone(),
                    fqdn
                );
                return true;
            }
            field == field_part && class == class_part
        })
    }
}
