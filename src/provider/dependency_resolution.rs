use std::collections::HashSet;
use std::fmt::Debug;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::{anyhow, Error, Ok, Result};
use fs_extra::dir::get_size;
use stack_graphs::graph::StackGraph;
use stack_graphs::partial::PartialPath;
use stack_graphs::partial::PartialPaths;
use stack_graphs::stitching::ForwardPartialPathStitcher;
use stack_graphs::stitching::StitcherConfig;
use stack_graphs::storage::SQLiteReader;
use stack_graphs::storage::SQLiteWriter;
use stack_graphs::NoCancellation;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::task::JoinSet;
use tracing::{debug, error, info, trace};
use tree_sitter_stack_graphs::loader::FileAnalyzers;

use crate::c_sharp_graph::dependency_xml_analyzer::DepXMLFileAnalyzer;
use crate::c_sharp_graph::language_config::SourceNodeLanguageConfiguration;
use crate::c_sharp_graph::loader::add_dir_to_graph;
use crate::c_sharp_graph::loader::AsyncInitializeGraph;
use crate::c_sharp_graph::loader::SourceType;
use crate::provider::project::Tools;
use crate::provider::AnalysisMode;
use crate::provider::Project;

const REFERNCE_ASSEMBLIES_NAME: &str = "Microsoft.NETFramework.ReferenceAssemblies";
pub struct Dependencies {
    pub location: PathBuf,
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub version: String,
    pub highest_restriction: String,
    pub decompiled_size: Mutex<Option<u64>>,
    pub decompiled_location: Arc<Mutex<HashSet<PathBuf>>>,
}

impl Debug for Dependencies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("\nDependencies")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("location", &self.location)
            .finish()
    }
}

impl Dependencies {
    pub async fn decompile(
        &self,
        reference_assmblies: PathBuf,
        restriction: String,
        tools: &Tools,
    ) -> Result<(), Error> {
        info!("decompiling dependency: {:?}", self);
        let dep_package_dir = self.location.to_owned();
        if !dep_package_dir.is_dir() || !dep_package_dir.exists() {
            return Err(anyhow!("invalid package path: {:?}", dep_package_dir));
        }
        let mut entries = fs::read_dir(dep_package_dir).await?;
        let mut paket_cache_file: Option<PathBuf> = None;
        while let Some(entry) = entries.next_entry().await? {
            // Find the paket_installmodel.cache file to read
            // and find the .dll's
            if entry.file_name().to_string_lossy() == "paket-installmodel.cache" {
                paket_cache_file = Some(entry.path());
                break;
            }
        }
        let to_decompile_locations = match paket_cache_file {
            Some(cache_file) => {
                // read_cache_file to get the path to the last found dll
                // this is an aproximation of what we want and eventually
                // we will need to understand the packet.dependencies file
                self.read_packet_cache_file(cache_file, restriction).await?
            }
            None => {
                debug!("did not find a cache file for dep: {:?}", self);
                return Err(anyhow!("did not find a cache file for dep: {:?}", self));
            }
        };
        if to_decompile_locations.is_empty() {
            trace!("no dll's found for dependnecy: {:?}", self);
        }
        let mut decompiled_files: HashSet<PathBuf> = HashSet::new();
        for file_to_decompile in to_decompile_locations {
            let decompiled_file = self
                .decompile_file(
                    &reference_assmblies,
                    file_to_decompile,
                    tools.ilspy_cmd.clone(),
                )
                .await?;
            decompiled_files.insert(decompiled_file);
        }

        info!(
            "deompiled {} files for dependnecy: {:?}",
            decompiled_files.len(),
            self
        );
        let mut dir_size: u64 = 0;
        for dir_path in decompiled_files.iter() {
            dir_size += get_size(dir_path).unwrap_or_default();
        }
        let mut size_guard = self.decompiled_size.lock().unwrap();
        let _ = size_guard.insert(dir_size);
        drop(size_guard);

        let mut guard = self.decompiled_location.lock().unwrap();
        *guard = decompiled_files;
        drop(guard);

        Ok(())
    }

    async fn read_packet_cache_file(
        &self,
        file: PathBuf,
        restriction: String,
    ) -> Result<Vec<PathBuf>, Error> {
        let file = File::open(file).await;
        if let Err(e) = file {
            error!("unable to find error: {:?}", e);
            return Err(anyhow!(e));
        }
        let reader = BufReader::new(file.ok().unwrap());
        let mut lines = reader.lines();
        let mut dlls: Vec<String> = vec![];
        let top_of_version = format!("D: /lib/{}", restriction);
        let mut valid_dir_to_search = "".to_string();
        let mut valid_file_match_start = "".to_string();

        while let Some(line) = lines.next_line().await? {
            if line.contains("D: /lib/")
                && line <= top_of_version
                && (valid_file_match_start.is_empty() || line > valid_dir_to_search)
            {
                valid_file_match_start = line.replace("D:", "F:");
                valid_dir_to_search = line.clone();
                dlls = vec![];
            }
            if line.contains(".dll")
                && !valid_dir_to_search.is_empty()
                && line.starts_with(&valid_file_match_start)
            {
                dlls.push(line);
            }
        }
        let dll_paths: Vec<PathBuf> = dlls
            .iter()
            .map(|x| {
                let p = self.location.join(x.trim_start_matches("F: /"));
                if !p.exists() {
                    debug!("unable to find path: {:?}", p);
                }
                p
            })
            .collect();

        Ok(dll_paths)
    }

    async fn decompile_file(
        &self,
        reference_assmblies: &PathBuf,
        file_to_decompile: PathBuf,
        ilspycmd: PathBuf,
    ) -> Result<PathBuf, Error> {
        let decompile_name = match self.location.as_path().file_name() {
            Some(n) => {
                let mut x = n.to_owned().to_string_lossy().into_owned();
                x.push_str("-decompiled");
                x
            }
            None => return Err(anyhow!("unable to dependency name")),
        };
        let decompile_out_name = match file_to_decompile.parent() {
            Some(p) => p.join(decompile_name),
            None => {
                return Err(anyhow!("unable to get path"));
            }
        };
        let decompile_output = Command::new(ilspycmd)
            .arg("-o")
            .arg(&decompile_out_name)
            .arg("-r")
            .arg(reference_assmblies)
            .arg("--no-dead-code")
            .arg("--no-dead-stores")
            .arg("-lv")
            .arg("CSharp7_3")
            .arg("-p")
            .arg(&file_to_decompile)
            .current_dir(&self.location)
            .output()?;

        trace!("decompile output: {:?}", decompile_output);

        Ok(decompile_out_name)
    }

    pub async fn get_xml_files(&self) -> Result<Vec<PathBuf>, Error> {
        let dep_package_dir = self.location.to_owned();
        if !dep_package_dir.is_dir() || !dep_package_dir.exists() {
            return Err(anyhow!("invalid package path: {:?}", dep_package_dir));
        }
        let mut entries = fs::read_dir(dep_package_dir).await?;
        let mut paket_cache_file: Option<PathBuf> = None;
        while let Some(entry) = entries.next_entry().await? {
            // Find the paket_installmodel.cache file to read
            // and find the .dll's
            if entry.file_name().to_string_lossy() == "paket-installmodel.cache" {
                paket_cache_file = Some(entry.path());
                break;
            }
        }
        let to_decompile_locations = match paket_cache_file {
            Some(cache_file) => {
                // read_cache_file to get the path to the last found dll
                // this is an aproximation of what we want and eventually
                // we will need to understand the packet.dependencies file
                self.read_packet_cache_file(cache_file, self.highest_restriction.clone())
                    .await?
            }
            None => {
                debug!("did not find a cache file for dep: {:?}", self);
                return Err(anyhow!("did not find a cache file for dep: {:?}", self));
            }
        };
        if to_decompile_locations.is_empty() {
            trace!("no dll's found for dependnecy: {:?}", self);
            return Ok(vec![]);
        }
        let new_locations = to_decompile_locations
            .into_iter()
            .map(|mut loc| {
                loc.set_extension("xml");
                loc
            })
            .collect();
        Ok(new_locations)
    }
}

impl Project {
    #[tracing::instrument]
    pub async fn resolve(&self) -> Result<(), Error> {
        // determine if the paket.dependencies already exists, if it does then we don't need to
        // convert.
        let paket_deps_file = self.location.clone().join("paket.dependencies");

        if !paket_deps_file.exists() {
            // Fsourcoirst need to run packet.
            // Need to convert and download all DLL's
            //TODO: Add paket location as a provider specific config.
            let paket_output = Command::new(&self.tools.paket_cmd)
                .args(["convert-from-nuget", "-f"])
                .current_dir(&self.location)
                .output()?;
            if !paket_output.status.success() {
                //TODO: Consider a specific error type
                debug!(
                    "paket command not successful: {} --- {}",
                    String::from_utf8_lossy(&paket_output.stdout),
                    String::from_utf8_lossy(&paket_output.stderr)
                );
                return Err(Error::msg("paket command did not succeed"));
            }
        }

        let (reference_assembly_path, highest_restriction, deps) = self
            .read_packet_dependency_file(paket_deps_file.as_path())
            .await?;
        if deps.is_empty() {
            let mut d = self.dependencies.lock().await;
            *d = Some(deps);
            return Ok(());
        }

        debug!(
            "got: {:?} -- {:?}",
            reference_assembly_path, highest_restriction
        );
        let mut set = JoinSet::new();
        if self.analysis_mode == AnalysisMode::Full {
            for d in deps {
                let reference_assmblies = reference_assembly_path.clone();
                let restriction = highest_restriction.clone();
                let tools = self.tools.clone();
                set.spawn(async move {
                    let decomp = d.decompile(reference_assmblies, restriction, &tools).await;
                    if let Err(e) = decomp {
                        error!("could not decompile - {:?}", e);
                    }
                    d
                });
            }
            // reset deps, as all the deps should be moved into the threads.
            let mut deps = vec![];
            while let Some(res) = set.join_next().await {
                match res {
                    std::result::Result::Ok(d) => {
                        deps.push(d);
                    }
                    Err(e) => {
                        return Err(Error::new(e));
                    }
                }
            }
            deps.sort_by(|x, y| {
                y.decompiled_size
                    .lock()
                    .unwrap()
                    .cmp(&x.decompiled_size.lock().unwrap())
            });
            let mut d = self.dependencies.lock().await;
            *d = Some(deps);
        } else {
            let mut d = self.dependencies.lock().await;
            *d = Some(deps);
        }
        Ok(())
    }

    pub async fn load_to_database(&self) -> Result<(), Error> {
        let set = if self.analysis_mode == AnalysisMode::Full {
            self.load_to_database_full_analysis().await?
        } else {
            self.load_to_database_source_only().await?
        };
        for res in set.join_all().await {
            let (init_graph, dep_name) = match res {
                std::result::Result::Ok((i, dep_name)) => (i, dep_name),
                Err(e) => {
                    return Err(anyhow!(
                        "unable to get graph, project may not have been initialized: {}",
                        e
                    ));
                }
            };
            info!(
                "loaded {} files for dep: {:?} into database",
                init_graph.files_loaded, dep_name
            );
        }
        let mut graph_guard = self
            .graph
            .lock()
            .expect("project may not have been initialized");
        info!("adding all dependency and source to graph");
        let mut db_reader = SQLiteReader::open(&self.db_path)?;
        // Load graphs from project location
        info!("Loading project graphs from: {:?}", &self.location);
        db_reader.load_graphs_for_file_or_directory(&self.location, &NoCancellation)?;

        // Also load SDK XML files if target framework is set
        if let Some(sdk_path) = self.get_sdk_path() {
            info!("Target framework set, SDK path: {:?}", sdk_path);
            info!("SDK path exists: {}", sdk_path.exists());
            if sdk_path.exists() {
                info!("Loading SDK graphs from: {:?}", sdk_path);
                if let Err(e) = db_reader.load_graphs_for_file_or_directory(&sdk_path, &NoCancellation) {
                    error!("Failed to load SDK graphs: {}", e);
                } else {
                    info!("Successfully loaded SDK graphs from database");
                }
            } else {
                info!("SDK path does not exist yet, skipping SDK graph loading");
            }
        } else {
            info!("No target framework set, skipping SDK graph loading");
        }

        // Once you read the data back from the DB, you will not get the source information
        // This is not currently stored in the database
        // There may be a way to re-attach this but for now we will relay code-snipper.
        let (read_graph, partials, databse) = db_reader.get();
        let read_graph = read_graph.to_serializable();
        let mut new_graph = StackGraph::new();
        read_graph.load_into(&mut new_graph)?;
        debug!(
            "new graph: {:?}",
            databse.to_serializable(&new_graph, partials)
        );
        let _ = graph_guard.insert(new_graph);
        Ok(())
    }

    async fn load_to_database_source_only(
        &self,
    ) -> Result<JoinSet<Result<(AsyncInitializeGraph, String), Error>>, Error> {
        let shared_deps = Arc::clone(&self.dependencies);
        let mut x = shared_deps.lock().await;
        let mut set = JoinSet::new();

        if let Some(ref mut vec) = *x {
            // For each dependnecy in the list we will try and load the decompiled files
            // Into the stack graph database.
            for d in vec {
                // Look up the location of the xml file.
                let xml_files = d.get_xml_files().await?;
                for file in xml_files {
                    if !file.exists() {
                        // Fallback to decompile.
                        error!("unable to find xml file: {:?}", file);
                        continue;
                    }
                    // Use new type of loader, to handle this.
                    let db_path = self.db_path.clone();
                    let dep_name = d.name.clone();
                    set.spawn(async move {
                        info!(
                            "indexing dep: {} with xml file: {:?} into a graph",
                            &dep_name, &file
                        );
                        let mut graph = StackGraph::new();
                        // We need to make sure that the symols for source type are the first
                        // symbols, so that they match what is in the builtins.
                        let (_, dep_source_type_node) =
                            SourceType::load_symbols_into_graph(&mut graph);
                        // remove mutability
                        let graph = graph;
                        let mut source_lc = SourceNodeLanguageConfiguration::new(
                            &tree_sitter_stack_graphs::NoCancellation,
                        )?;
                        let file_name = file.file_name();
                        if file_name.is_none() {
                            return Err(anyhow!("unable to handle file"));
                        }
                        let file_name = file_name.unwrap().to_string_lossy();
                        let file_name = file_name.to_string();
                        source_lc.language_config.special_files =
                            FileAnalyzers::new().with(file_name, DepXMLFileAnalyzer {});
                        let mut graph = add_dir_to_graph(
                            &file,
                            &source_lc.dependnecy_type_node_info,
                            &source_lc.language_config,
                            graph,
                        )?;
                        let root_node = graph.stack_graph.iter_nodes().find(|n| {
                            let node = &graph.stack_graph[*n];
                            node.is_root()
                        });
                        if root_node.is_none() {
                            error!("unable to find root node");
                        }
                        let root_node = root_node.unwrap();
                        let dep_source_type_node = graph.stack_graph.iter_nodes().find(|n| {
                            let node = &graph.stack_graph[*n];
                            let symbol = node.symbol();
                            symbol.is_some()
                                && symbol.unwrap() == dep_source_type_node.get_symbol_handle()
                        });
                        let dep_source_type_node = dep_source_type_node.unwrap();
                        graph
                            .stack_graph
                            .add_edge(root_node, dep_source_type_node, 0);

                        let mut db: SQLiteWriter = SQLiteWriter::open(db_path)?;
                        for (file_path, tag) in graph.file_to_tag.clone() {
                            let file_str = file_path.to_string_lossy();
                            let file_handle = graph
                                .stack_graph
                                .get_file(&file_str)
                                .ok_or(anyhow!("unable to get file"))?;
                            let mut partials = PartialPaths::new();
                            let mut paths: Vec<PartialPath> = vec![];
                            let stats =
                                ForwardPartialPathStitcher::find_minimal_partial_path_set_in_file(
                                    &graph.stack_graph,
                                    &mut partials,
                                    file_handle,
                                    StitcherConfig::default().with_collect_stats(true),
                                    &NoCancellation,
                                    |_, _, p| paths.push(p.clone()),
                                )?;
                            db.store_result_for_file(
                                &graph.stack_graph,
                                file_handle,
                                &tag,
                                &mut partials,
                                &paths,
                            )?;
                            trace!("stats for stitiching: {:?} - paths: {}", stats, paths.len(),);
                        }
                        info!(
                            "stats for dependency: {:?}, files indexed {:?}",
                            dep_name, graph.files_loaded,
                        );
                        Ok((graph, dep_name))
                    });
                }
            }
        }
        Ok(set)
    }

    async fn load_to_database_full_analysis(
        &self,
    ) -> Result<JoinSet<Result<(AsyncInitializeGraph, String), Error>>, Error> {
        let shared_deps = Arc::clone(&self.dependencies);
        let mut x = shared_deps.lock().await;
        let mut set: JoinSet<Result<(AsyncInitializeGraph, String), Error>> = JoinSet::new();
        if let Some(ref mut vec) = *x {
            // For each dependnecy in the list we will try and load the decompiled files
            // Into the stack graph database.
            for d in vec {
                let size = d.decompiled_size.lock().unwrap().unwrap_or_default();
                let decompiled_locations: Arc<Mutex<HashSet<PathBuf>>> =
                    Arc::clone(&d.decompiled_location);
                let decompiled_locations = decompiled_locations.lock().unwrap();
                let decompiled_files = &(*decompiled_locations);
                for decompiled_file in decompiled_files {
                    let file = decompiled_file.clone();
                    let lc = self.source_language_config.clone();
                    let db_path = self.db_path.clone();
                    let dep_name = d.name.clone();
                    set.spawn(async move {
                        info!(
                            "indexing dep: {} with size: {} into a graph",
                            dep_name, &size
                        );
                        let mut graph = StackGraph::new();
                        // We need to make sure that the symols for source type are the first
                        // symbols, so that they match what is in the builtins.
                        let (_, _) = SourceType::load_symbols_into_graph(&mut graph);
                        // remove mutability
                        let graph = graph;
                        let lc_guard = lc.read().await;
                        let lc = match lc_guard.as_ref() {
                            Some(x) => x,
                            None => {
                                return Err(anyhow!("unable to get source language config"));
                            }
                        };

                        let graph = add_dir_to_graph(
                            &file,
                            &lc.dependnecy_type_node_info,
                            &lc.language_config,
                            graph,
                        )?;
                        drop(lc_guard);
                        let mut db: SQLiteWriter = SQLiteWriter::open(db_path)?;
                        for (file_path, tag) in graph.file_to_tag.clone() {
                            let file_str = file_path.to_string_lossy();
                            let file_handle = graph
                                .stack_graph
                                .get_file(&file_str)
                                .ok_or(anyhow!("unable to get file"))?;
                            let mut partials = PartialPaths::new();
                            let mut paths: Vec<PartialPath> = vec![];
                            let stats =
                                ForwardPartialPathStitcher::find_minimal_partial_path_set_in_file(
                                    &graph.stack_graph,
                                    &mut partials,
                                    file_handle,
                                    StitcherConfig::default().with_collect_stats(true),
                                    &NoCancellation,
                                    |_, _, p| paths.push(p.clone()),
                                )?;
                            db.store_result_for_file(
                                &graph.stack_graph,
                                file_handle,
                                &tag,
                                &mut partials,
                                &paths,
                            )?;
                            trace!("stats for stitiching: {:?} - paths: {}", stats, paths.len(),);
                        }
                        debug!(
                            "stats for dependency: {:?}, files indexed {:?}",
                            dep_name, graph.files_loaded,
                        );
                        Ok((graph, dep_name))
                    });
                }
            }
        }
        Ok(set)
    }

    async fn read_packet_dependency_file(
        &self,
        paket_deps_file: &Path,
    ) -> Result<(PathBuf, String, Vec<Dependencies>), Error> {
        let file = File::open(paket_deps_file).await;
        if let Err(e) = file {
            error!("unable to find error: {:?}", e);
            return Err(anyhow!(e));
        }
        let reader = BufReader::new(file.ok().unwrap());
        let mut lines = reader.lines();
        let mut smallest_framework = "zzzzzzzzzzzzzzz".to_string();
        let mut deps: Vec<Dependencies> = vec![];
        while let Some(line) = lines.next_line().await? {
            if !line.contains("restriction") {
                continue;
            }
            let parts: Vec<&str> = line.split("restriction:").collect();
            if parts.len() != 2 {
                continue;
            }
            if let Some(dep_part) = parts.first() {
                let white_space_split: Vec<&str> = dep_part.split_whitespace().collect();
                if white_space_split.len() < 4 {
                    continue;
                }
                let mut dep_path = self.location.clone();
                dep_path.push("packages");
                let name = match white_space_split.get(1) {
                    Some(n) => n,
                    None => {
                        continue;
                    }
                };
                dep_path.push(name);
                let version = match white_space_split.get(2) {
                    Some(v) => v,
                    None => {
                        continue;
                    }
                };
                let dep = Dependencies {
                    location: dep_path,
                    name: name.to_string(),
                    version: version.to_string(),
                    decompiled_location: Arc::new(Mutex::new(HashSet::new())),
                    decompiled_size: Mutex::new(None),
                    highest_restriction: "".to_string(),
                };
                deps.push(dep);
            }

            if let Some(ref_name) = parts.get(1) {
                let n = ref_name.to_string();
                if let Some(framework) = n.split_whitespace().last() {
                    let framework_string = framework.to_string();
                    if framework_string < smallest_framework {
                        smallest_framework = framework_string;
                    }
                }
            }
        }
        drop(lines);

        let deps: Vec<Dependencies> = deps
            .into_iter()
            .map(|mut d| {
                d.highest_restriction = smallest_framework.clone();
                d
            })
            .collect();

        if deps.is_empty() {
            return Ok((PathBuf::new(), String::new(), deps));
        }

        // Now we we have the framework, we need to get the reference_assmblies
        let base_name = format!("{}.{}", REFERNCE_ASSEMBLIES_NAME, smallest_framework);
        let paket_reference_output = Command::new(&self.tools.paket_cmd)
            .args(["add", base_name.as_str()])
            .current_dir(&self.location)
            .output()?;

        debug!("paket_reference_output: {:?}", paket_reference_output);

        let paket_install = match paket_deps_file.parent() {
            Some(dir) => dir.to_path_buf().join("packages").join(base_name),
            None => {
                return Err(anyhow!(
                    "unable to find the paket install of reference assembly"
                ));
            }
        };
        // Read the paket_install to find the directory of the DLL's
        let file = File::open(paket_install.join("paket-installmodel.cache")).await;
        if let Err(e) = file {
            error!("unable to find error: {:?}", e);
            return Err(anyhow!(e));
        }
        let reader = BufReader::new(file.ok().unwrap());
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            if line.contains("build/.NETFramework/") && line.contains("D: /") {
                let path_str = match line.strip_prefix("D: /") {
                    Some(x) => x,
                    None => {
                        return Err(anyhow!("unable to get reference assembly"));
                    }
                };
                debug!("path_str: {}", path_str);
                let path = paket_install.join(path_str);
                return Ok((paket_install.join(path), smallest_framework, deps));
            }
        }

        Err(anyhow!("unable to get reference assembly"))
    }

    /// Load SDK XML files into the database
    /// Processes all files in a single graph build and database write
    /// Returns the count of successfully loaded files
    pub async fn load_sdk_xml_files_to_database(
        &self,
        xml_files: Vec<PathBuf>,
    ) -> Result<usize, Error> {
        info!("Loading {} SDK XML files into database", xml_files.len());

        if xml_files.is_empty() {
            return Ok(0);
        }

        // Filter out non-existent files
        let valid_files: Vec<PathBuf> = xml_files
            .into_iter()
            .filter(|file| {
                if !file.exists() {
                    error!("SDK XML file does not exist: {:?}", file);
                    false
                } else {
                    true
                }
            })
            .collect();

        if valid_files.is_empty() {
            return Ok(0);
        }

        // Create a single graph for all XML files
        let mut graph = StackGraph::new();

        // Load source type symbols into graph
        let (_, dep_source_type_node) = SourceType::load_symbols_into_graph(&mut graph);
        let graph = graph; // Remove mutability

        // Create language configuration with all XML file analyzers
        let mut source_lc =
            SourceNodeLanguageConfiguration::new(&tree_sitter_stack_graphs::NoCancellation)?;

        // Register all XML files with the analyzer
        let mut file_analyzers = FileAnalyzers::new();
        for file in &valid_files {
            let file_name = file
                .file_name()
                .ok_or_else(|| anyhow!("unable to get file name for {:?}", file))?
                .to_string_lossy()
                .to_string();
            file_analyzers = file_analyzers.with(file_name, DepXMLFileAnalyzer {});
        }
        source_lc.language_config.special_files = file_analyzers;

        // Process all files into the same graph
        let mut current_graph = graph;
        let mut success_count = 0;
        let mut combined_file_to_tag = std::collections::HashMap::new();

        for file in &valid_files {
            info!("Indexing SDK XML file: {:?} into graph", file);
            match add_dir_to_graph(
                file,
                &source_lc.dependnecy_type_node_info,
                &source_lc.language_config,
                current_graph,
            ) {
                std::result::Result::Ok(result) => {
                    success_count += result.files_loaded;
                    current_graph = result.stack_graph;
                    // Accumulate file_to_tag mappings
                    for (path, tag) in result.file_to_tag {
                        combined_file_to_tag.insert(path, tag);
                    }
                }
                Err(e) => {
                    error!("Failed to add SDK XML file {:?} to graph: {}", file, e);
                    // Continue with the rest of the files - need to recreate a graph
                    current_graph = StackGraph::new();
                    let (_, _) = SourceType::load_symbols_into_graph(&mut current_graph);
                }
            }
        }

        if success_count == 0 {
            return Err(anyhow!("Failed to process any SDK XML files"));
        }

        // Connect root node to dependency source type node
        let root_node = current_graph.iter_nodes().find(|n| {
            let node = &current_graph[*n];
            node.is_root()
        });

        if root_node.is_none() {
            error!("Unable to find root node in combined graph");
            return Err(anyhow!("unable to find root node"));
        }
        let root_node = root_node.unwrap();

        let dep_source_type_node_handle = current_graph.iter_nodes().find(|n| {
            let node = &current_graph[*n];
            let symbol = node.symbol();
            symbol.is_some() && symbol.unwrap() == dep_source_type_node.get_symbol_handle()
        });

        if let Some(dep_node) = dep_source_type_node_handle {
            current_graph.add_edge(root_node, dep_node, 0);
        }

        // Write all results to database in a single operation
        info!("Writing {} SDK XML files to database", success_count);
        let mut db: SQLiteWriter = SQLiteWriter::open(&self.db_path)?;

        for (file_path, tag) in combined_file_to_tag {
            let file_str = file_path.to_string_lossy();
            let file_handle = current_graph
                .get_file(&file_str)
                .ok_or_else(|| anyhow!("unable to get file"))?;

            let mut partials = PartialPaths::new();
            let mut paths: Vec<PartialPath> = vec![];
            let stats = ForwardPartialPathStitcher::find_minimal_partial_path_set_in_file(
                &current_graph,
                &mut partials,
                file_handle,
                StitcherConfig::default().with_collect_stats(true),
                &NoCancellation,
                |_, _, p| paths.push(p.clone()),
            )?;

            db.store_result_for_file(&current_graph, file_handle, &tag, &mut partials, &paths)?;

            trace!(
                "Stats for stitching SDK XML {}: {:?} - paths: {}",
                file_str,
                stats,
                paths.len()
            );
        }

        info!(
            "SDK XML loading complete: {} files successfully processed",
            success_count
        );
        Ok(success_count)
    }
}
