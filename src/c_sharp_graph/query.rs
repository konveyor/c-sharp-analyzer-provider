use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    vec,
};

use anyhow::{Error, Ok};
use regex::Regex;
use serde_json::Value;
use stack_graphs::{
    arena::Handle,
    graph::{Edge, File, Node, StackGraph},
};
use tracing::{debug, error, info, trace};
use url::Url;

use crate::c_sharp_graph::{
    class_query::ClassSymbolsGetter,
    field_query::FieldSymbolsGetter,
    loader::SourceType,
    method_query::MethodSymbolsGetter,
    namespace_query::NamespaceSymbolsGetter,
    results::{Location, Position, ResultNode},
};

pub trait Query {
    fn query(self, query: String) -> anyhow::Result<Vec<ResultNode>, Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxType {
    Import,
    CompUnit,
    NamespaceDeclaration,
    ClassDef,
    MethodName,
    FieldName,
    LocalVar,
    Argument,
    Name,
}

impl SyntaxType {
    pub(crate) fn get(syntax_type_string: &str) -> Self {
        match syntax_type_string {
            "import" => Self::Import,
            "comp_unit" => Self::CompUnit,
            "namespace_declaration" => Self::NamespaceDeclaration,
            "class_def" => Self::ClassDef,
            "method_name" => Self::MethodName,
            "field_name" => Self::FieldName,
            "local_var" => Self::LocalVar,
            "argument" => Self::Argument,
            "name" => Self::Name,
            // Name is the least used thing, and I want to have a default for this.
            &_ => Self::Name,
        }
    }
    pub(crate) fn to_string(&self) -> &str {
        match self {
            Self::Import => "import",
            Self::CompUnit => "comp_unit",
            Self::NamespaceDeclaration => "namespace_declaration",
            Self::ClassDef => "class_def",
            Self::MethodName => "method_name",
            Self::FieldName => "field_name",
            Self::LocalVar => "local_var",
            Self::Argument => "argument",
            Self::Name => "name",
        }
    }
}

#[derive(Clone, Eq, Hash, PartialEq, Debug, Ord, PartialOrd)]
pub(crate) struct Fqdn {
    pub(crate) namespace: Option<String>,
    pub(crate) class: Option<String>,
    pub(crate) method: Option<String>,
    pub(crate) field: Option<String>,
}

pub(crate) fn get_fqdn(node: Handle<Node>, graph: &StackGraph) -> Option<Fqdn> {
    let mut fqdn = Fqdn {
        namespace: None,
        class: None,
        method: None,
        field: None,
    };
    // traverse upwards based on the FQDN edge
    // Once there is no FQDN edge, return
    let n = &graph[node];
    let source_info = graph
        .source_info(node)
        .expect("FQDN nodes must have source info");
    let syntax_type = &graph[source_info
        .syntax_type
        .into_option()
        .expect("FQDN nodes must have a syntax type")];
    let syntax_type = SyntaxType::get(syntax_type);
    // if this node that is from a FQDN does not have a symobl something is
    // very wrong in the TSG.
    let symbol_handle = n.symbol().unwrap();
    let symbol = graph[symbol_handle].to_string();
    // Collect and sort edges to ensure deterministic selection
    let mut fqdn_edges: Vec<_> = graph
        .outgoing_edges(node)
        .filter(|e| e.precedence == 10)
        .collect();
    fqdn_edges.sort_by_key(|e| e.sink);
    let fqdn_edge = fqdn_edges.first().copied();
    match fqdn_edge {
        None => match syntax_type {
            SyntaxType::NamespaceDeclaration => {
                fqdn.namespace = Some(symbol);
                Some(fqdn)
            }
            SyntaxType::MethodName => {
                fqdn.method = Some(symbol);
                Some(fqdn)
            }
            SyntaxType::ClassDef => {
                fqdn.class = Some(symbol);
                Some(fqdn)
            }
            SyntaxType::FieldName => {
                fqdn.field = Some(symbol);
                Some(fqdn)
            }
            _ => None,
        },
        Some(e) => match get_fqdn(e.sink, graph) {
            None => Some(fqdn),
            Some(mut f) => match syntax_type {
                SyntaxType::NamespaceDeclaration => {
                    f.namespace = f.namespace.map_or_else(
                        || Some(symbol.clone()),
                        |n| Some(format!("{}.{}", n, symbol.clone())),
                    );
                    Some(f)
                }
                SyntaxType::MethodName => {
                    f.method = f.method.map_or_else(
                        || Some(symbol.clone()),
                        |m| Some(format!("{}.{}", m, symbol.clone())),
                    );
                    Some(f)
                }
                SyntaxType::ClassDef => {
                    f.class = f.class.map_or_else(
                        || Some(symbol.clone()),
                        |c| Some(format!("{}.{}", c, symbol.clone())),
                    );
                    Some(f)
                }
                SyntaxType::FieldName => {
                    f.field = f.field.map_or_else(
                        || Some(symbol.clone()),
                        |field| Some(format!("{}.{}", field, symbol.clone())),
                    );
                    Some(f)
                }
                _ => None,
            },
        },
    }
}

pub enum QueryType<'graph> {
    All {
        graph: &'graph StackGraph,
        source_type: &'graph SourceType,
    },
    Method {
        graph: &'graph StackGraph,
        source_type: &'graph SourceType,
    },
    Field {
        graph: &'graph StackGraph,
        source_type: &'graph SourceType,
    },
    Class {
        graph: &'graph StackGraph,
        source_type: &'graph SourceType,
    },
}

impl Query for QueryType<'_> {
    fn query(self, query: String) -> anyhow::Result<Vec<ResultNode>, Error> {
        match self {
            QueryType::All { graph, source_type } => {
                let q = Querier {
                    graph,
                    source_type,
                    _matcher_getter: NamespaceSymbolsGetter {},
                };
                q.query(query)
            }
            QueryType::Method { graph, source_type } => {
                info!("running method search");
                let q = Querier {
                    graph,
                    source_type,
                    _matcher_getter: MethodSymbolsGetter {},
                };
                q.query(query)
            }
            QueryType::Field { graph, source_type } => {
                let q = Querier {
                    graph,
                    source_type,
                    _matcher_getter: FieldSymbolsGetter {},
                };
                q.query(query)
            }
            QueryType::Class { graph, source_type } => {
                let q = Querier {
                    graph,
                    source_type,
                    _matcher_getter: ClassSymbolsGetter {},
                };
                q.query(query)
            }
        }
    }
}

pub(crate) struct Querier<'graph, T: GetMatcher> {
    pub(crate) graph: &'graph StackGraph,
    pub(crate) source_type: &'graph SourceType,
    _matcher_getter: T,
}

#[derive(Debug)]
pub(crate) struct StartingNodes {
    definition_root_nodes: Vec<Handle<Node>>,
    referenced_files: BTreeSet<Handle<File>>,
    file_to_compunit_handle: BTreeMap<Handle<File>, Handle<Node>>,
}

impl<T: GetMatcher> Querier<'_, T> {
    pub(crate) fn get_search(&self, query: String) -> anyhow::Result<Search, Error> {
        Search::create_search(query)
    }

    pub(crate) fn get_starting_nodes(&self, search: &Search) -> StartingNodes {
        // get all the compilation units that use some portion of the search (using System or
        // using System.Configuration) This will require us to then determine if there qualified
        // names ConfigurationManager.AppSettings for examples;

        // We will also need to find the definition of this by looking at the namespace
        // declaration. then we need to capture all the nodes that are definitions attached to
        // this (for instance namespace System.Configuration; Class ConfigurationManager; method
        // AppSettings)
        let mut definition_root_nodes: Vec<Handle<Node>> = vec![];
        let mut referenced_files: BTreeSet<Handle<File>> = BTreeSet::new();
        let mut file_to_compunit_handle: BTreeMap<Handle<File>, Handle<Node>> = BTreeMap::new();

        for node_handle in self.graph.iter_nodes() {
            let node: &Node = &self.graph[node_handle];
            let file_handle = match node.file() {
                Some(h) => h,
                None => {
                    continue;
                }
            };
            let symbol_option = node.symbol();
            if symbol_option.is_none() {
                // If the node doesn't have a symbol to look at, then we should continue and it
                // only used to tie together other nodes.
                //
                trace!("node no symbol: {}", node.display(self.graph));
                continue;
            }
            let symbol = &self.graph[node.symbol().unwrap()];
            let source_info = self.graph.source_info(node_handle);
            if source_info.is_none() {
                trace!("node no source_info: {}", node.display(self.graph));
                continue;
            }
            match source_info.unwrap().syntax_type.into_option() {
                None => continue,
                Some(handle) => {
                    let syntax_type = SyntaxType::get(&self.graph[handle]);
                    trace!(
                        "node for syntax_type: {} -- syntax_type: {:?}",
                        node.display(self.graph),
                        syntax_type
                    );
                    match syntax_type {
                        SyntaxType::CompUnit => {
                            file_to_compunit_handle.insert(file_handle, node_handle);
                        }
                        SyntaxType::Import => {
                            if search.partial_namespace(symbol) {
                                referenced_files.insert(file_handle);
                            }
                        }
                        SyntaxType::NamespaceDeclaration => {
                            if search.match_namespace(symbol) {
                                definition_root_nodes.push(node_handle);
                                referenced_files.insert(file_handle);
                            }
                        }
                        _ => continue,
                    }
                }
            }
        }

        // Sort definition_root_nodes to ensure deterministic query results
        definition_root_nodes.sort();

        StartingNodes {
            definition_root_nodes,
            referenced_files,
            file_to_compunit_handle,
        }
    }

    pub(crate) fn search_nodes(
        &self,
        file: Handle<File>,
        symbol_matcher: &T::Matcher,
        searchable_nodes: &BTreeSet<Handle<Node>>,
        results: &mut Vec<ResultNode>,
        file_uri: String,
    ) {
        let mut searchable_nodes = searchable_nodes.clone();
        searchable_nodes.extend(self.graph.nodes_for_file(file));
        debug!(
            file = %file_uri,
            "searchable nodes: {:?} for already found: {}",
            searchable_nodes.len(),
            results.len(),
        );
        let mut used_nodes: BTreeSet<Handle<Node>> = BTreeSet::new();
        // Collect and sort nodes to ensure deterministic iteration
        let mut file_nodes: Vec<Handle<Node>> = self.graph.nodes_for_file(file).collect();
        file_nodes.sort();
        let file_nodes_count = file_nodes.len();
        for node_handle in file_nodes {
            if used_nodes.contains(&node_handle) {
                continue;
            }
            let node = &self.graph[node_handle];
            let symbol = match node.symbol() {
                None => {
                    used_nodes.insert(node_handle);
                    continue;
                }
                Some(handle) => &self.graph[handle],
            };

            let source_info = match self.graph.source_info(node_handle) {
                Some(s) => s,
                None => {
                    used_nodes.insert(node_handle);
                    continue;
                }
            };

            let mut full_symbol: Option<Fqdn> = None;
            if node.is_reference() {
                full_symbol = self.get_type_with_symbol(node_handle, symbol, &searchable_nodes);
                if full_symbol.is_none() {
                    trace!(
                        file = %file_uri,
                        "unable to get full symbol: {}",
                        node_handle.display(self.graph),
                    );
                    used_nodes.insert(node_handle);
                    continue;
                }
                let full_symbol_unwrap = full_symbol.clone().unwrap();
                trace!("found FQDN: {:?}", &full_symbol);
                if !symbol_matcher.match_fqdn(&full_symbol_unwrap) {
                    used_nodes.insert(node_handle);
                    continue;
                }
            } else if !symbol_matcher.match_symbol(symbol.to_string()) {
                used_nodes.insert(node_handle);
                continue;
            }
            let debug_node = self.graph.node_debug_info(node_handle).map_or(vec![], |d| {
                d.iter()
                    .map(|e| {
                        let k = self.graph[e.key].to_string();
                        let v = self.graph[e.value].to_string();
                        (k, v)
                    })
                    .collect()
            });

            let line_number = source_info.span.start.line;
            let code_location = Location {
                start_position: Position {
                    line: source_info.span.start.line,
                    character: source_info.span.start.column.utf8_offset,
                },
                end_position: Position {
                    line: source_info.span.end.line,
                    character: source_info.span.end.column.utf8_offset,
                },
            };

            // Get syntax type information for the node
            let mut syntax_type_str = source_info.syntax_type.into_option().map_or_else(
                || "unknown".to_string(),
                |st| {
                    let st_symbol = &self.graph[st];
                    st_symbol.to_string()
                },
            );

            // Create variables map with file and syntax_type
            let mut var: BTreeMap<String, Value> = BTreeMap::new();
            var.insert("file".to_string(), Value::from(file_uri.clone()));

            // Add symbol for debugging
            var.insert("symbol".to_string(), Value::from(symbol.to_string()));

            // Add FQDN for debugging and infer syntax_type for references
            if node.is_reference() && full_symbol.is_some() {
                let fqdn = full_symbol.unwrap();
                if let Some(ns) = &fqdn.namespace {
                    var.insert("fqdn_namespace".to_string(), Value::from(ns.clone()));
                }
                if let Some(cls) = &fqdn.class {
                    var.insert("fqdn_class".to_string(), Value::from(cls.clone()));
                }
                if let Some(method) = &fqdn.method {
                    var.insert("fqdn_method".to_string(), Value::from(method.clone()));
                    // Infer syntax_type from FQDN structure if it was unknown
                    if syntax_type_str == "unknown" {
                        syntax_type_str = "method_reference".to_string();
                    }
                }
                if let Some(field) = &fqdn.field {
                    var.insert("fqdn_field".to_string(), Value::from(field.clone()));
                    // Infer syntax_type from FQDN structure if it was unknown
                    if syntax_type_str == "unknown" {
                        syntax_type_str = "field_reference".to_string();
                    }
                }
            }

            // Set syntax_type after potential inference from FQDN
            var.insert("syntax_type".to_string(), Value::from(syntax_type_str));

            trace!(file=%file_uri, "found result for node: {:?}", debug_node);
            results.push(ResultNode {
                file_uri: file_uri.clone(),
                line_number,
                code_location,
                variables: var,
            });
            used_nodes.insert(node_handle);
        }
        if used_nodes.len() != file_nodes_count {
            error!(
                file = %file_uri,
                "Not all file nodes were processed! used_nodes: {}, file_nodes: {}",
                used_nodes.len(),
                file_nodes_count
            );
        }
        debug!(
            file = %file_uri,
            "searchable nodes: {:?} for found after searching file nodes: {}",
            searchable_nodes.len(),
            results.len(),
        );
    }

    // Select the best FQDN from candidates based on imports
    // Strategy:
    // 1. Prefer FQDNs whose namespace exactly matches an import
    // 2. Prefer FQDNs whose namespace is imported (partial match)
    // 3. Fall back to first candidate for determinism
    fn select_best_fqdn(&self, candidates: Vec<Fqdn>, imports: BTreeSet<String>) -> Option<Fqdn> {
        if candidates.is_empty() {
            return None;
        }

        if candidates.len() == 1 {
            debug!("Only one candidate, returning: {:?}", candidates[0]);
            if let Some(ref ns) = candidates[0].namespace {
                if imports.contains(ns) {
                    debug!(
                        "Selected candidate with exact namespace match: {:?}",
                        candidates[0]
                    );
                    return Some(candidates[0].clone());
                }
            }
            return None;
        }

        debug!(
            "Selecting from {} candidates with {} imports",
            candidates.len(),
            imports.len()
        );

        for candidate in &candidates {
            if let Some(ref ns) = candidate.namespace {
                if imports.contains(ns) {
                    debug!(
                        "Selected candidate with exact namespace match: {:?}",
                        candidate
                    );
                    return Some(candidate.clone());
                }
            }
        }

        // No import match found - return None for consistency
        debug!(
            "No import match found for {} candidates, returning None",
            candidates.len()
        );
        None
    }

    // Helper function to get all imported namespaces for a file
    fn get_file_imports(&self, file: Handle<File>) -> BTreeSet<String> {
        let mut imports = BTreeSet::new();

        for node_handle in self.graph.nodes_for_file(file) {
            let node = &self.graph[node_handle];

            // Check if this is an import node
            let source_info = match self.graph.source_info(node_handle) {
                Some(s) => s,
                None => continue,
            };

            match source_info.syntax_type.into_option() {
                None => continue,
                Some(handle) => {
                    let syntax_type = SyntaxType::get(&self.graph[handle]);
                    if syntax_type == SyntaxType::Import {
                        if let Some(symbol_handle) = node.symbol() {
                            let namespace = self.graph[symbol_handle].to_string();
                            imports.insert(namespace);
                        }
                    }
                }
            }
        }

        imports
    }

    // Note: This function will only work, on the memeber_access_expresssion
    fn get_type_with_symbol(
        &self,
        node: Handle<Node>,
        symbol: &str,
        searchable_nodes: &BTreeSet<Handle<Node>>,
    ) -> Option<Fqdn> {
        let parts: Vec<&str> = symbol.split(".").collect();
        if parts.len() != 2 {
            return None;
        }
        let accessed_part = parts
            .last()
            .expect("unable to get method part for symbol")
            .to_string();
        let accessor = parts
            .first()
            .expect("unable to get class part for symbol")
            .to_string();

        // Get imports for the file containing this reference
        let access_node = &self.graph[node];
        let file = access_node.file()?;
        let imports = self.get_file_imports(file);

        debug!(
            "Resolving {}.{} with {} imports: {:?}",
            accessor,
            accessed_part,
            imports.len(),
            imports
        );

        // Find the symbol that matches the accessor
        // Collect and sort to ensure deterministic selection
        let mut matching_symbols: Vec<_> = self
            .graph
            .iter_symbols()
            .filter(|s| accessor == self.graph[*s])
            .collect();
        matching_symbols.sort();
        let get_symbol_handle_for_accessor = matching_symbols.first().copied()?;
        // Find the node that is the defintion of the symbol.
        let mut nodes_for_defines_symbol: Vec<&Handle<Node>> = searchable_nodes
            .iter()
            .filter(|f| {
                let n = &self.graph[**f];
                if !n.is_definition() {
                    return false;
                }
                let s = n.symbol();
                if s.is_none() {
                    return false;
                }

                s.unwrap() == get_symbol_handle_for_accessor
            })
            .collect();
        // Sort to ensure deterministic selection when multiple definitions match
        nodes_for_defines_symbol.sort();

        trace!(
            "looking for correct definition for {}-{}, found {} candidates",
            accessor,
            accessed_part,
            nodes_for_defines_symbol.len()
        );

        // Collect all candidate FQDNs with their namespaces
        let mut candidates: Vec<Fqdn> = vec![];

        for definition_node in nodes_for_defines_symbol {
            let source_info = &self.graph.source_info(*definition_node);
            if source_info.is_none() {
                continue;
            }
            let syntax_type = source_info.unwrap().syntax_type;
            if syntax_type.is_none() {
                trace!(
                    "no syntax_type for node: {}",
                    definition_node.display(self.graph)
                );
                continue;
            }
            let syntax_type = syntax_type.into_option().unwrap();
            let syntax_type = &self.graph[syntax_type];

            let fqdn = match SyntaxType::get(syntax_type) {
                SyntaxType::ClassDef => {
                    // Collect and sort edges to ensure deterministic selection
                    let mut matching_edges: Vec<_> = self
                        .graph
                        .outgoing_edges(*definition_node)
                        .filter(|e| {
                            let sink = &self.graph[e.sink];
                            trace!("testing sink: {}", sink.display(self.graph));
                            match sink.symbol() {
                                Some(sym) => self.graph[sym] == accessed_part,
                                None => false,
                            }
                        })
                        .collect();
                    matching_edges.sort_by_key(|e| e.sink);
                    let found_edge = matching_edges.first();
                    found_edge.and_then(|e| get_fqdn(e.sink, self.graph))
                }
                SyntaxType::FieldName | SyntaxType::MethodName => {
                    get_fqdn(*definition_node, self.graph)
                }
                SyntaxType::LocalVar => {
                    access_node.file().and_then(|f| {
                        let fqdns = self.get_local_var_type_fqdn(
                            *definition_node,
                            &accessed_part,
                            f,
                            searchable_nodes,
                        );
                        candidates.extend(fqdns);
                        None::<Fqdn>
                    });
                    None
                    // When the symbol is defined by a local variable
                    // then we need to find the local var type.
                }
                _ => None,
            };
            trace!(
                "found candidate: {:?} for node: {}",
                fqdn,
                definition_node.display(self.graph)
            );
            if let Some(fqdn) = fqdn {
                candidates.push(fqdn);
            }
        }

        // Now prefer FQDNs whose namespace matches an import
        self.select_best_fqdn(candidates, imports)
    }

    fn get_local_var_type_fqdn(
        &self,
        definition_node: Handle<Node>,
        accessed_part_symbol: &str,
        file: Handle<File>,
        searchable_nodes: &BTreeSet<Handle<Node>>,
    ) -> Vec<Fqdn> {
        let def_node = &self.graph[definition_node];
        if !def_node.is_in_file(file) {
            return vec![];
        }
        let type_ref_node = self.graph.outgoing_edges(definition_node).find_map(|e| {
            let edge_node = &self.graph[e.sink];
            if edge_node.is_reference() {
                Some(edge_node)
            } else {
                None
            }
        });
        if type_ref_node.is_none() {
            return vec![];
        }
        let type_ref_node = type_ref_node.unwrap();

        let ref_symbol = type_ref_node.symbol();
        if ref_symbol.is_none() {
            return vec![];
        }
        let ref_symbol = ref_symbol.unwrap();
        trace!(
            "searching for defintion for type_ref_node: {}",
            type_ref_node.display(self.graph)
        );
        // Sort searchable_nodes for deterministic iteration
        let mut sorted_nodes: Vec<_> = searchable_nodes.iter().copied().collect();
        sorted_nodes.sort();

        let defined_node: Vec<Handle<Node>> = sorted_nodes
            .iter()
            .flat_map(|x| {
                let node = &self.graph[*x];
                if node.symbol().is_none() || node.symbol().unwrap() != ref_symbol {
                    return None;
                }
                trace!(
                "found defined node, checking edges for symbols that match the accessed_part: {}",
                node.display(self.graph)
                );
                // Collect and sort edges to ensure deterministic selection
                let matching_edges: Vec<_> = self
                    .graph
                    .outgoing_edges(*x)
                    .filter_map(|e| {
                        let sink = &self.graph[e.sink];
                        trace!("testing sink: {}", sink.display(self.graph));
                        match sink.symbol() {
                            Some(sym) => {
                                if &self.graph[sym] == accessed_part_symbol {
                                    Some(e.sink)
                                } else {
                                    None
                                }
                            }
                            None => None,
                        }
                    })
                    .collect();
                Some(matching_edges)
            })
            .flatten()
            .collect();

        defined_node
            .iter()
            .filter_map(|x| get_fqdn(*x, self.graph))
            .collect()
    }
}

impl<T: GetMatcher> Query for Querier<'_, T> {
    fn query(self, query: String) -> anyhow::Result<Vec<ResultNode>, Error> {
        let search: Search = self.get_search(query)?;

        debug!("search: {:?}", search);

        let mut results: Vec<ResultNode> = vec![];

        let starting_nodes = self.get_starting_nodes(&search);

        // Get all the definition_node_handles to be searched.
        let mut searchable_nodes: BTreeSet<Handle<Node>> = BTreeSet::new();
        for definition_root_node in &starting_nodes.definition_root_nodes {
            let root_node = &self.graph[*definition_root_node];
            let def_nodes = self
                .graph
                .nodes_for_file(root_node.file().unwrap())
                .filter(|n| {
                    let def_node = &self.graph[*n];
                    if def_node.is_definition() {
                        return true;
                    }
                    false
                });
            searchable_nodes.extend(def_nodes);
        }

        // Now that we have the all the nodes we need to build the reference symbols to match the *
        let symbol_matcher =
            T::get_matcher(self.graph, starting_nodes.definition_root_nodes, &search)?;

        let (is_source, symbol_handle) = match self.source_type {
            SourceType::Source { symbol_handle } => (true, Some(symbol_handle)),
            _ => (false, None),
        };

        info!(
            "searching for match: {:?} across {} files",
            search,
            starting_nodes.referenced_files.len()
        );
        for file in starting_nodes.referenced_files.iter() {
            let comp_unit_node_handle = match starting_nodes.file_to_compunit_handle.get(file) {
                Some(x) => x,
                None => {
                    error!(
                        "unable to find compulation unit for file: {}",
                        file.display(self.graph)
                    );
                    continue;
                }
            };
            // This determines if the file is source code or not, but using the source_type symbol
            // graph node.
            if is_source
                && !self.graph.nodes_for_file(*file).any(|node_handle| {
                    let node = &self.graph[node_handle];

                    let symobl_handle = symbol_handle.unwrap();
                    if let Some(sh) = node.symbol() {
                        // This compares the source_type symbol handle to the nodes symbol
                        // as symbols are de-duplicated, this will check that the symbol for the
                        // given node is the one that we set for the source_type in the graph.
                        if sh.as_usize() == symobl_handle.as_usize() {
                            if self.source_type.get_string() != self.graph[sh] {
                                error!("SOMETHING IS VERY WRONG!!!!");
                            }
                            // We need to make sure that the compulation unit for the file is
                            // actually has an edge from teh source_type node.
                            let edges: Vec<Edge> = self.graph.outgoing_edges(node_handle).collect();
                            for edge in edges {
                                if edge.sink == *comp_unit_node_handle {
                                    return true;
                                }
                            }
                        }
                    }
                    false
                })
            {
                continue;
            }
            let f = &self.graph[*file];
            let mut file_str = f.name().to_string();
            let file_path = Path::new(f.name());
            if !file_path.is_absolute() {
                file_str = format!("/{}", file_str).clone();
            }
            let file_url = Url::from_file_path(&file_str);
            if file_url.is_err() {
                error!("unable to get the file url for file: {}", &file_str);
                continue;
            }
            let file_uri = file_url.unwrap().as_str().to_string();
            self.search_nodes(
                *file,
                &symbol_matcher,
                &searchable_nodes,
                &mut results,
                file_uri,
            );
        }
        // Sort results to ensure deterministic output
        results.sort();

        // Log results for determinism debugging
        let pattern = search
            .parts
            .iter()
            .map(|p| p.part.as_str())
            .collect::<Vec<_>>()
            .join(".");
        info!(
            "Query results for pattern '{}': {} incidents",
            pattern,
            results.len()
        );
        for (i, result) in results.iter().enumerate() {
            let symbol = result
                .variables
                .get("symbol")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let fqdn_ns = result
                .variables
                .get("fqdn_namespace")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let fqdn_class = result
                .variables
                .get("fqdn_class")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let fqdn_field = result
                .variables
                .get("fqdn_field")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let fqdn_display =
                if !fqdn_ns.is_empty() || !fqdn_class.is_empty() || !fqdn_field.is_empty() {
                    format!(" fqdn={}.{}.{}", fqdn_ns, fqdn_class, fqdn_field)
                } else {
                    String::new()
                };

            debug!(
                "  Result[{}]: {} line {} symbol={}{}",
                i, result.file_uri, result.line_number, symbol, fqdn_display
            );
        }

        Ok(results)
    }
}

pub(crate) trait GetMatcher {
    type Matcher: SymbolMatcher;
    fn get_matcher(
        stack_graphs: &StackGraph,
        definition_root_nodes: Vec<Handle<Node>>,
        search: &Search,
    ) -> Result<Self::Matcher, Error>
    where
        Self: std::marker::Sized;
}

pub(crate) trait SymbolMatcher {
    fn match_symbol(&self, symbol: String) -> bool;
    fn match_fqdn(&self, fqdn: &Fqdn) -> bool;
}

#[derive(Debug)]
struct SearchPart {
    part: String,
    regex: Option<Regex>,
}

#[derive(Debug)]
pub(crate) struct Search {
    parts: Vec<SearchPart>,
}

impl Search {
    pub(crate) fn create_search(query: String) -> anyhow::Result<Search, Error> {
        let mut parts: Vec<SearchPart> = vec![];
        let star_regex = Regex::new(".*")?;

        let mut in_regex_string = false;
        let mut building_regex_string = String::new();
        for part in query.split(".") {
            let mut new_part = part.to_string();
            if part.contains("(") {
                in_regex_string = true;
                building_regex_string = new_part;
                continue;
            }
            if in_regex_string && part.contains(")") {
                new_part = format!("{}.{}", building_regex_string, part);
                in_regex_string = false;
            } else if in_regex_string {
                building_regex_string = format!("{}.{}", building_regex_string, part);
                continue;
            }

            let regex = if new_part == "*" {
                // Pure wildcard - match anything
                Some(star_regex.clone())
            } else if new_part.contains('*')
                || new_part.contains('(')
                || new_part.contains(')')
                || new_part.contains('|')
            {
                // Contains regex metacharacters - treat as regex pattern
                let pattern = if new_part == ".*" {
                    // Already in regex form, don't replace
                    new_part.to_string()
                } else if new_part.contains('*') {
                    // Replace * with .* for wildcard matching
                    new_part.replace("*", ".*")
                } else {
                    // Has other metacharacters but no *, use as-is
                    new_part.to_string()
                };
                Some(Regex::new(&format!("^{}$", pattern))?)
            } else {
                // Plain string - use exact matching (no regex)
                None
            };

            parts.push(SearchPart {
                part: part.to_string(),
                regex,
            });
        }

        Ok(Search { parts })
    }
}

impl Search {
    pub(crate) fn partial_namespace(&self, symbol: &str) -> bool {
        // We will need to break apart the symbol based on "." then looping through, look at the
        // same index, and if it matches continue if it doesn't then return false.
        for (i, symbol_part) in symbol.split(".").enumerate() {
            if self.parts.len() <= i {
                break;
            }
            if !self.parts[i].matches(symbol_part) {
                return false;
            }
        }
        true
    }

    pub(crate) fn match_namespace(&self, symbol: &str) -> bool {
        for (i, symbol_part) in symbol.split(".").enumerate() {
            // Because we can assume that the last part here is a '*' right now,
            // we anything past that should match
            if self.parts.len() <= i {
                break;
            }
            if !self.parts[i].matches(symbol_part) {
                return false;
            }
        }
        true
    }

    pub(crate) fn match_symbol(&self, symbol: &str) -> bool {
        let last_part = match self.parts.last() {
            None => {
                return false;
            }
            Some(x) => x,
        };
        last_part.matches(symbol)
    }

    // fn import_match
    //Namespace Match
    //Part Match
    //Regex Match
    //???
}

impl SearchPart {
    fn matches(&self, match_string: &str) -> bool {
        match &self.regex {
            None => self.part == match_string,
            Some(r) => r.is_match(match_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for Search::create_search()

    #[test]
    fn test_create_search_simple_query() {
        let search = Search::create_search("System.Configuration".to_string()).unwrap();
        assert_eq!(search.parts.len(), 2);
        assert_eq!(search.parts[0].part, "System");
        assert_eq!(search.parts[1].part, "Configuration");
    }

    #[test]
    fn test_create_search_with_wildcard() {
        let search = Search::create_search("System.*".to_string()).unwrap();
        assert_eq!(search.parts.len(), 2);
        assert_eq!(search.parts[0].part, "System");
        assert_eq!(search.parts[1].part, "*");

        // Verify wildcard matches anything
        assert!(search.parts[1].matches("Configuration"));
        assert!(search.parts[1].matches("IO"));
        assert!(search.parts[1].matches(""));
    }

    #[test]
    fn test_create_search_single_part() {
        let search = Search::create_search("System".to_string()).unwrap();
        assert_eq!(search.parts.len(), 1);
        assert_eq!(search.parts[0].part, "System");
    }

    #[test]
    fn test_create_search_multiple_wildcards() {
        let search = Search::create_search("*.*.*".to_string()).unwrap();
        assert_eq!(search.parts.len(), 3);
        assert_eq!(search.parts[0].part, "*");
        assert_eq!(search.parts[1].part, "*");
        assert_eq!(search.parts[2].part, "*");
    }

    #[test]
    fn test_create_search_empty_string() {
        let search = Search::create_search("".to_string()).unwrap();
        assert_eq!(search.parts.len(), 1);
        assert_eq!(search.parts[0].part, "");
    }

    #[test]
    fn test_create_search_three_parts() {
        let search = Search::create_search("System.Configuration.Manager".to_string()).unwrap();
        assert_eq!(search.parts.len(), 3);
        assert_eq!(search.parts[0].part, "System");
        assert_eq!(search.parts[1].part, "Configuration");
        assert_eq!(search.parts[2].part, "Manager");
    }

    // Tests for SearchPart::matches()

    #[test]
    fn test_search_part_matches_exact_without_regex() {
        let part = SearchPart {
            part: "System".to_string(),
            regex: None,
        };
        assert!(part.matches("System"));
        assert!(!part.matches("Configuration"));
        assert!(!part.matches(""));
    }

    #[test]
    fn test_search_part_matches_with_regex() {
        let regex = Regex::new("(System|Configuration)").unwrap();
        let part = SearchPart {
            part: "(System|Configuration)".to_string(),
            regex: Some(regex),
        };
        assert!(part.matches("System"));
        assert!(part.matches("Configuration"));
        assert!(!part.matches("IO"));
    }

    #[test]
    fn test_search_part_matches_wildcard_regex() {
        let regex = Regex::new(".*").unwrap();
        let part = SearchPart {
            part: "*".to_string(),
            regex: Some(regex),
        };
        assert!(part.matches("anything"));
        assert!(part.matches(""));
        assert!(part.matches("System.Configuration"));
    }

    #[test]
    fn test_search_part_matches_case_sensitive() {
        let regex = Regex::new("System").unwrap();
        let part = SearchPart {
            part: "System".to_string(),
            regex: Some(regex),
        };
        assert!(part.matches("System"));
        assert!(!part.matches("system"));
        assert!(!part.matches("SYSTEM"));
    }

    // Tests for Search::partial_namespace()

    #[test]
    fn test_partial_namespace_exact_match() {
        let search = Search::create_search("System.Configuration".to_string()).unwrap();
        assert!(search.partial_namespace("System.Configuration"));
    }

    #[test]
    fn test_partial_namespace_symbol_longer() {
        let search = Search::create_search("System".to_string()).unwrap();
        assert!(search.partial_namespace("System.Configuration.Manager"));
    }

    #[test]
    fn test_partial_namespace_symbol_shorter() {
        let search = Search::create_search("System.Configuration.Manager".to_string()).unwrap();
        assert!(search.partial_namespace("System.Configuration"));
        assert!(search.partial_namespace("System"));
    }

    #[test]
    fn test_partial_namespace_no_match() {
        let search = Search::create_search("System.Configuration".to_string()).unwrap();
        assert!(!search.partial_namespace("Other.Namespace"));
        assert!(!search.partial_namespace("System.IO"));
    }

    #[test]
    fn test_partial_namespace_with_wildcard() {
        let search = Search::create_search("System.*".to_string()).unwrap();
        assert!(search.partial_namespace("System.Configuration"));
        assert!(search.partial_namespace("System.IO"));
        assert!(!search.partial_namespace("Other.Configuration"));
    }

    #[test]
    fn test_partial_namespace_empty_symbol() {
        let search = Search::create_search("System".to_string()).unwrap();
        assert!(!search.partial_namespace(""));
    }

    #[test]
    fn test_partial_namespace_prefix_match() {
        let search = Search::create_search("System.Configuration".to_string()).unwrap();
        assert!(search.partial_namespace("System.Configuration.Manager"));
        assert!(search.partial_namespace("System.Configuration"));
        assert!(!search.partial_namespace("System.IO"));
    }

    // Tests for Search::match_namespace()

    #[test]
    fn test_match_namespace_exact_match() {
        let search = Search::create_search("System.Configuration".to_string()).unwrap();
        assert!(search.match_namespace("System.Configuration"));
    }

    #[test]
    fn test_match_namespace_symbol_longer() {
        let search = Search::create_search("System".to_string()).unwrap();
        assert!(search.match_namespace("System.Configuration.Manager"));
    }

    #[test]
    fn test_match_namespace_symbol_shorter() {
        let search = Search::create_search("System.Configuration.Manager".to_string()).unwrap();
        assert!(search.match_namespace("System.Configuration"));
        assert!(search.match_namespace("System"));
    }

    #[test]
    fn test_match_namespace_no_match() {
        let search = Search::create_search("System.Configuration".to_string()).unwrap();
        assert!(!search.match_namespace("Other.Namespace"));
        assert!(!search.match_namespace("System.IO"));
    }

    #[test]
    fn test_match_namespace_with_wildcard() {
        let search = Search::create_search("System.*".to_string()).unwrap();
        assert!(search.match_namespace("System.Configuration"));
        assert!(search.match_namespace("System.IO"));
        assert!(!search.match_namespace("Other.Configuration"));
    }

    #[test]
    fn test_match_namespace_empty_symbol() {
        let search = Search::create_search("System".to_string()).unwrap();
        assert!(!search.match_namespace(""));
    }

    // Tests for Search::match_symbol()

    #[test]
    fn test_match_symbol_matches_last_part() {
        let search = Search::create_search("System.Configuration.AppSettings".to_string()).unwrap();
        assert!(search.match_symbol("AppSettings"));
        assert!(!search.match_symbol("Configuration"));
        assert!(!search.match_symbol("System"));
    }

    #[test]
    fn test_match_symbol_with_wildcard() {
        let search = Search::create_search("System.*".to_string()).unwrap();
        assert!(search.match_symbol("Configuration"));
        assert!(search.match_symbol("IO"));
        assert!(search.match_symbol("anything"));
    }

    #[test]
    fn test_match_symbol_exact_match() {
        let search = Search::create_search("System.Configuration.AppSettings".to_string()).unwrap();
        assert!(search.match_symbol("AppSettings"));
        assert!(!search.match_symbol("Configuration"));
        assert!(!search.match_symbol("Manager"));
    }

    #[test]
    fn test_match_symbol_single_part_search() {
        let search = Search::create_search("AppSettings".to_string()).unwrap();
        assert!(search.match_symbol("AppSettings"));
        assert!(!search.match_symbol("Other"));
    }

    #[test]
    fn test_match_symbol_empty_search() {
        let search = Search::create_search("".to_string()).unwrap();
        // Empty string creates one part with empty string, which uses regex ".*"
        // The empty part's regex will match empty string but the part field is ""
        // so it should match empty but also anything due to regex
        assert!(search.match_symbol(""));
    }

    // Integration tests combining multiple functions

    #[test]
    fn test_full_query_workflow_simple() {
        let search =
            Search::create_search("System.Configuration.ConfigurationManager".to_string()).unwrap();

        // Test partial_namespace
        assert!(search.partial_namespace("System.Configuration.ConfigurationManager"));
        assert!(search.partial_namespace("System.Configuration"));
        assert!(search.partial_namespace("System"));

        // Test match_namespace
        assert!(search.match_namespace("System.Configuration.ConfigurationManager"));
        assert!(search.match_namespace("System.Configuration"));

        // Test match_symbol
        assert!(search.match_symbol("ConfigurationManager"));
        assert!(!search.match_symbol("Configuration"));
    }

    #[test]
    fn test_full_query_workflow_with_wildcards() {
        let search = Search::create_search("System.*.*".to_string()).unwrap();

        // Test partial_namespace
        assert!(search.partial_namespace("System.Configuration.ConfigurationManager"));
        assert!(search.partial_namespace("System.IO.File"));
        assert!(!search.partial_namespace("Other.Namespace"));

        // Test match_symbol (last part is wildcard)
        assert!(search.match_symbol("anything"));
        assert!(search.match_symbol("File"));
    }

    #[test]
    fn test_full_query_workflow_multi_level() {
        let search = Search::create_search(
            "System.Configuration.ConfigurationManager.AppSettings".to_string(),
        )
        .unwrap();

        // Test partial_namespace - should match prefixes
        assert!(search.partial_namespace("System.Configuration.ConfigurationManager.AppSettings"));
        assert!(search.partial_namespace("System.Configuration.ConfigurationManager"));
        assert!(search.partial_namespace("System.Configuration"));
        assert!(search.partial_namespace("System"));
        assert!(!search.partial_namespace("Other.Namespace"));

        // Test match_namespace - same behavior as partial for this case
        assert!(search.match_namespace("System.Configuration.ConfigurationManager.AppSettings"));
        assert!(search.match_namespace("System.Configuration"));

        // Test match_symbol (last part is "AppSettings")
        assert!(search.match_symbol("AppSettings"));
        assert!(!search.match_symbol("ConfigurationManager"));
        assert!(!search.match_symbol("Configuration"));
    }

    // Tests for select_best_fqdn logic
    // Note: These tests use a mock Querier to test the selection logic

    use crate::c_sharp_graph::loader::SourceType;
    use stack_graphs::graph::StackGraph;

    #[test]
    fn test_select_best_fqdn_single_candidate_not_in_import() {
        let mut graph = StackGraph::new();
        let source_type = SourceType::Source {
            symbol_handle: graph.add_symbol("source"),
        };
        let querier = Querier {
            graph: &graph,
            source_type: &source_type,
            _matcher_getter: NamespaceSymbolsGetter {},
        };

        let candidates = vec![Fqdn {
            namespace: Some("System.Configuration".to_string()),
            class: Some("ConfigurationManager".to_string()),
            method: Some("AppSettings".to_string()),
            field: None,
        }];
        let imports = BTreeSet::new();

        let result = querier.select_best_fqdn(candidates, imports);
        assert!(result.is_none());
    }
    #[test]
    fn test_select_best_fqdn_single_candidate_not_ns_in_imports() {
        let mut graph = StackGraph::new();
        let source_type = SourceType::Source {
            symbol_handle: graph.add_symbol("source"),
        };
        let querier = Querier {
            graph: &graph,
            source_type: &source_type,
            _matcher_getter: NamespaceSymbolsGetter {},
        };

        let candidates = vec![Fqdn {
            namespace: Some("System.Configuration".to_string()),
            class: Some("ConfigurationManager".to_string()),
            method: Some("AppSettings".to_string()),
            field: None,
        }];
        let mut imports = BTreeSet::new();
        imports.insert("System.Configuration.Test".to_string());

        let result = querier.select_best_fqdn(candidates, imports);
        assert!(result.is_none());
    }

    #[test]
    fn test_select_best_fqdn_exact_namespace_match() {
        let mut graph = StackGraph::new();
        let source_type = SourceType::Source {
            symbol_handle: graph.add_symbol("source"),
        };
        let querier = Querier {
            graph: &graph,
            source_type: &source_type,
            _matcher_getter: NamespaceSymbolsGetter {},
        };

        let candidates = vec![
            Fqdn {
                namespace: Some("MyApp.Configuration".to_string()),
                class: Some("ConfigurationManager".to_string()),
                method: Some("AppSettings".to_string()),
                field: None,
            },
            Fqdn {
                namespace: Some("System.Configuration".to_string()),
                class: Some("ConfigurationManager".to_string()),
                method: Some("AppSettings".to_string()),
                field: None,
            },
        ];
        let mut imports = BTreeSet::new();
        imports.insert("System.Configuration".to_string());

        let result = querier.select_best_fqdn(candidates, imports);
        assert!(result.is_some());
        // Should select the one with exact namespace match
        assert_eq!(
            result.unwrap().namespace,
            Some("System.Configuration".to_string())
        );
    }

    #[test]
    fn test_select_best_fqdn_partial_namespace_no_match() {
        let mut graph = StackGraph::new();
        let source_type = SourceType::Source {
            symbol_handle: graph.add_symbol("source"),
        };
        let querier = Querier {
            graph: &graph,
            source_type: &source_type,
            _matcher_getter: NamespaceSymbolsGetter {},
        };

        let candidates = vec![
            Fqdn {
                namespace: Some("MyApp.Configuration".to_string()),
                class: Some("ConfigurationManager".to_string()),
                method: Some("AppSettings".to_string()),
                field: None,
            },
            Fqdn {
                namespace: Some("System.Configuration.Internal".to_string()),
                class: Some("ConfigurationManager".to_string()),
                method: Some("AppSettings".to_string()),
                field: None,
            },
        ];
        let mut imports = BTreeSet::new();
        imports.insert("System.Configuration".to_string());

        let result = querier.select_best_fqdn(candidates, imports);
        assert!(result.is_none());
    }

    #[test]
    fn test_select_best_fqdn_no_match_returns_none() {
        let mut graph = StackGraph::new();
        let source_type = SourceType::Source {
            symbol_handle: graph.add_symbol("source"),
        };
        let querier = Querier {
            graph: &graph,
            source_type: &source_type,
            _matcher_getter: NamespaceSymbolsGetter {},
        };

        let candidates = vec![
            Fqdn {
                namespace: Some("AAA.Configuration".to_string()),
                class: Some("ConfigurationManager".to_string()),
                method: None,
                field: None,
            },
            Fqdn {
                namespace: Some("ZZZ.Configuration".to_string()),
                class: Some("ConfigurationManager".to_string()),
                method: None,
                field: None,
            },
        ];
        let mut imports = BTreeSet::new();
        imports.insert("Other.Namespace".to_string());

        let result = querier.select_best_fqdn(candidates, imports);
        // Should return None when no import matches
        assert!(result.is_none());
    }

    #[test]
    fn test_select_best_fqdn_empty_candidates() {
        let mut graph = StackGraph::new();
        let source_type = SourceType::Source {
            symbol_handle: graph.add_symbol("source"),
        };
        let querier = Querier {
            graph: &graph,
            source_type: &source_type,
            _matcher_getter: NamespaceSymbolsGetter {},
        };

        let candidates = vec![];
        let imports = BTreeSet::new();

        let result = querier.select_best_fqdn(candidates, imports);
        assert!(result.is_none());
    }

    #[test]
    fn test_select_best_fqdn_prefers_exact_over_partial() {
        let mut graph = StackGraph::new();
        let source_type = SourceType::Source {
            symbol_handle: graph.add_symbol("source"),
        };
        let querier = Querier {
            graph: &graph,
            source_type: &source_type,
            _matcher_getter: NamespaceSymbolsGetter {},
        };

        let candidates = vec![
            Fqdn {
                namespace: Some("System.Configuration.Internal".to_string()),
                class: Some("ConfigurationManager".to_string()),
                method: None,
                field: None,
            },
            Fqdn {
                namespace: Some("System.Configuration".to_string()),
                class: Some("ConfigurationManager".to_string()),
                method: None,
                field: None,
            },
        ];
        let mut imports = BTreeSet::new();
        imports.insert("System.Configuration".to_string());

        let result = querier.select_best_fqdn(candidates, imports);
        assert!(result.is_some());
        // Should prefer exact match over partial match
        assert_eq!(
            result.unwrap().namespace,
            Some("System.Configuration".to_string())
        );
    }
}
