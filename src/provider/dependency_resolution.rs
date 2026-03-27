use std::collections::HashSet;
use std::fmt::Debug;
use std::path::Path;
use std::path::PathBuf;
use tokio::process::Command;
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
use tracing::{debug, error, info, instrument, trace};
use crate::provider::telemetry::METRICS;
use tree_sitter_stack_graphs::loader::FileAnalyzers;

use crate::c_sharp_graph::dependency_xml_analyzer::DepXMLFileAnalyzer;
use crate::c_sharp_graph::language_config::SourceNodeLanguageConfiguration;
use crate::c_sharp_graph::loader::add_dir_to_graph;
use crate::c_sharp_graph::loader::AsyncInitializeGraph;
use crate::c_sharp_graph::loader::SourceType;
use crate::provider::project::Tools;
use crate::provider::target_framework::TargetFrameworkHelper;
use crate::provider::AnalysisMode;
use crate::provider::Project;

/// Result type for JoinSet tasks that build graphs for individual dependencies.
type GraphBuildResult = Result<(AsyncInitializeGraph, String), Error>;

const REFERENCE_ASSEMBLIES_NAME: &str = "Microsoft.NETFramework.ReferenceAssemblies";
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
    #[instrument(skip_all, name = "deps.decompile", fields(dep = %self.name))]
    pub async fn decompile(
        &self,
        reference_assemblies: PathBuf,
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
            trace!("no dll's found for dependency: {:?}", self);
        }
        let mut decompiled_files: HashSet<PathBuf> = HashSet::new();
        for file_to_decompile in to_decompile_locations {
            let decompiled_file = self
                .decompile_file(
                    &reference_assemblies,
                    file_to_decompile,
                    tools.ilspy_cmd.clone(),
                )
                .await?;
            decompiled_files.insert(decompiled_file);
        }

        info!(
            "decompiled {} files for dependency: {:?}",
            decompiled_files.len(),
            self
        );
        let mut dir_size: u64 = 0;
        for dir_path in decompiled_files.iter() {
            dir_size += get_size(dir_path).unwrap_or_default();
        }
        match self.decompiled_size.lock() {
            std::result::Result::Ok(mut guard) => { guard.replace(dir_size); }
            Err(e) => error!("decompiled_size mutex poisoned: {}", e),
        }

        match self.decompiled_location.lock() {
            std::result::Result::Ok(mut guard) => { *guard = decompiled_files; }
            Err(e) => error!("decompiled_location mutex poisoned: {}", e),
        }

        Ok(())
    }

    #[instrument(skip_all, name = "deps.read_cache_file")]
    async fn read_packet_cache_file(
        &self,
        file: PathBuf,
        restriction: String,
    ) -> Result<Vec<PathBuf>, Error> {
        let file = File::open(file).await.map_err(|e| {
            error!("unable to open file: {:?}", e);
            anyhow!(e)
        })?;
        let reader = BufReader::new(file);
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

    #[instrument(skip_all, name = "deps.decompile_file", fields(file = %file_to_decompile.display()))]
    async fn decompile_file(
        &self,
        reference_assemblies: &PathBuf,
        file_to_decompile: PathBuf,
        ilspycmd: PathBuf,
    ) -> Result<PathBuf, Error> {
        let _timer = METRICS.decompile_duration_seconds.start_timer();
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
            .arg(reference_assemblies)
            .arg("--no-dead-code")
            .arg("--no-dead-stores")
            .arg("-lv")
            .arg("CSharp7_3")
            .arg("-p")
            .arg(&file_to_decompile)
            .current_dir(&self.location)
            .output()
            .await?;

        trace!("decompile output: {:?}", decompile_output);

        Ok(decompile_out_name)
    }

    #[instrument(skip_all, name = "deps.get_xml_files")]
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
            trace!("no dll's found for dependency: {:?}", self);
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
    #[instrument(skip_all, name = "deps.resolve")]
    pub async fn resolve(&self) -> Result<(), Error> {
        // determine if the paket.dependencies already exists, if it does then we don't need to
        // convert.
        let paket_deps_file = self.location.join("paket.dependencies");

        if !paket_deps_file.exists() {
            // First need to run paket.
            // Need to convert and download all DLL's
            //TODO: Add paket location as a provider specific config.
            let paket_output = Command::new(&self.tools.paket_cmd)
                .args(["convert-from-nuget", "-f"])
                .current_dir(&self.location)
                .output()
                .await?;
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
                let reference_assemblies = reference_assembly_path.clone();
                let restriction = highest_restriction.clone();
                let tools = self.tools.clone();
                set.spawn(async move {
                    let decomp = d.decompile(reference_assemblies, restriction, &tools).await;
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
                let y_size = y.decompiled_size.lock().ok().and_then(|g| *g);
                let x_size = x.decompiled_size.lock().ok().and_then(|g| *g);
                y_size.cmp(&x_size)
            });
            let mut d = self.dependencies.lock().await;
            *d = Some(deps);
        } else {
            let mut d = self.dependencies.lock().await;
            *d = Some(deps);
        }
        Ok(())
    }

    #[instrument(skip_all, name = "deps.load_to_database")]
    pub async fn load_to_database(&self) -> Result<(), Error> {
        if self.analysis_mode == AnalysisMode::Full {
            self.load_to_database_full_analysis().await?;
        } else {
            self.load_to_database_source_only().await?;
        }
        // Perform all I/O and computation WITHOUT holding the graph lock
        info!("adding all dependency and source to graph");
        let mut db_reader = SQLiteReader::open(&self.db_path)?;
        // Load graphs from project location
        info!("Loading project graphs from: {:?}", &self.location);
        db_reader.load_graphs_for_file_or_directory(&self.location, &NoCancellation)?;

        // Also load SDK XML files if target framework is set
        if let Some(sdk_path) = self.get_sdk_path() {
            info!("Target framework set, SDK path: {:?}", sdk_path);
            if sdk_path.exists() {
                info!("Loading SDK graphs from: {:?}", sdk_path);
                if let Err(e) =
                    db_reader.load_graphs_for_file_or_directory(&sdk_path, &NoCancellation)
                {
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
        let (read_graph, partials, database) = db_reader.get();
        let read_graph = read_graph.to_serializable();
        let mut new_graph = StackGraph::new();
        read_graph.load_into(&mut new_graph)?;
        debug!(
            "new graph: {:?}",
            database.to_serializable(&new_graph, partials)
        );

        // Only acquire the lock to swap in the new graph
        let mut graph_guard = self.graph.lock()
            .unwrap_or_else(|e| e.into_inner());
        graph_guard.replace(new_graph);
        Ok(())
    }

    #[instrument(skip_all, name = "deps.load_to_db_source_only")]
    async fn load_to_database_source_only(&self) -> Result<(), Error> {
        let shared_deps = Arc::clone(&self.dependencies);
        let mut x = shared_deps.lock().await;
        let mut set = JoinSet::new();

        if let Some(ref mut vec) = *x {
            for d in vec {
                let xml_files = d.get_xml_files().await?;
                for file in xml_files {
                    if !file.exists() {
                        error!("unable to find xml file: {:?}", file);
                        continue;
                    }
                    let db_path = self.db_path.clone();
                    let dep_name = d.name.clone();
                    let span = tracing::info_span!("deps.index_dep", dep = %dep_name);
                    // Use spawn_blocking since this task does heavy blocking I/O
                    // (filesystem traversal, tree-sitter parsing, SQLite writes)
                    set.spawn_blocking(move || {
                        let _guard = span.enter();
                        info!(
                            "indexing dep: {} with xml file: {:?} into a graph",
                            &dep_name, &file
                        );
                        let mut graph = StackGraph::new();
                        let (_, dep_source_type_node) =
                            SourceType::load_symbols_into_graph(&mut graph);
                        let graph = graph;
                        let mut source_lc = SourceNodeLanguageConfiguration::new(
                            &tree_sitter_stack_graphs::NoCancellation,
                        )?;
                        let Some(file_name) = file.file_name() else {
                            return Err(anyhow!("unable to handle file"));
                        };
                        let file_name = file_name.to_string_lossy().to_string();
                        source_lc.language_config.special_files =
                            FileAnalyzers::new().with(file_name, DepXMLFileAnalyzer {});
                        let mut graph = add_dir_to_graph(
                            &file,
                            &source_lc.dependency_type_node_info,
                            &source_lc.language_config,
                            graph,
                        )?;
                        let root_node = graph.stack_graph.iter_nodes().find(|n| {
                            let node = &graph.stack_graph[*n];
                            node.is_root()
                        });
                        let root_node = root_node.ok_or_else(|| anyhow!("unable to find root node"))?;
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
                            trace!("stats for stitching: {:?} - paths: {}", stats, paths.len(),);
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
        // Wait for all tasks and report results within this span
        Self::join_graph_tasks(set).await
    }

    #[instrument(skip_all, name = "deps.load_to_db_full")]
    async fn load_to_database_full_analysis(&self) -> Result<(), Error> {
        let shared_deps = Arc::clone(&self.dependencies);
        let mut x = shared_deps.lock().await;
        let mut set: JoinSet<GraphBuildResult> = JoinSet::new();
        if let Some(ref mut vec) = *x {
            for d in vec {
                let size = d.decompiled_size.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .unwrap_or_default();
                let decompiled_locations = d.decompiled_location.lock()
                    .unwrap_or_else(|e| e.into_inner());
                for decompiled_file in decompiled_locations.iter() {
                    let file = decompiled_file.clone();
                    let lc = self.source_language_config.clone();
                    let db_path = self.db_path.clone();
                    let dep_name = d.name.clone();
                    let span = tracing::info_span!("deps.index_dep", dep = %dep_name, size = size);
                    // Acquire an owned read guard in the async context, then move it
                    // into spawn_blocking. OwnedRwLockReadGuard is Send + 'static,
                    // and the read lock allows concurrent tasks to share access.
                    set.spawn(async move {
                        let lc_guard = Arc::clone(&lc).read_owned().await;
                        tokio::task::spawn_blocking(move || {
                            let _guard = span.enter();
                            info!(
                                "indexing dep: {} with size: {} into a graph",
                                dep_name, &size
                            );
                            let lc = lc_guard.as_ref()
                                .ok_or_else(|| anyhow!("unable to get source language config"))?;

                            let mut graph = StackGraph::new();
                            let (_, _) = SourceType::load_symbols_into_graph(&mut graph);
                            let graph = add_dir_to_graph(
                                &file,
                                &lc.dependency_type_node_info,
                                &lc.language_config,
                                graph,
                            )?;
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
                                trace!("stats for stitching: {:?} - paths: {}", stats, paths.len(),);
                            }
                            debug!(
                                "stats for dependency: {:?}, files indexed {:?}",
                                dep_name, graph.files_loaded,
                            );
                            Ok((graph, dep_name))
                        })
                        .await
                        .map_err(|e| anyhow!("blocking task panicked: {}", e))?
                    });
                }
            }
        }
        // Wait for all tasks and report results within this span
        Self::join_graph_tasks(set).await
    }

    /// Wait for all graph build tasks and log results.
    async fn join_graph_tasks(set: JoinSet<GraphBuildResult>) -> Result<(), Error> {
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
        Ok(())
    }

    #[instrument(skip_all, name = "deps.read_packet_dep_file")]
    async fn read_packet_dependency_file(
        &self,
        paket_deps_file: &Path,
    ) -> Result<(PathBuf, String, Vec<Dependencies>), Error> {
        let file = File::open(paket_deps_file).await.map_err(|e| {
            error!("unable to open file: {:?}", e);
            anyhow!(e)
        })?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let mut smallest_framework: Option<String> = None;
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
                    if smallest_framework.as_ref().map_or(true, |sf| framework_string < *sf) {
                        smallest_framework = Some(framework_string);
                    }
                }
            }
        }
        let deps: Vec<Dependencies> = deps
            .into_iter()
            .map(|mut d| {
                d.highest_restriction = smallest_framework.clone().unwrap_or_default();
                d
            })
            .collect();

        if deps.is_empty() {
            return Ok((PathBuf::new(), String::new(), deps));
        }

        // Now we we have the framework, we need to get the reference_assemblies
        let base_name = format!("{}.{}", REFERENCE_ASSEMBLIES_NAME, smallest_framework.as_deref().unwrap_or_default());
        let paket_reference_output = Command::new(&self.tools.paket_cmd)
            .args(["add", base_name.as_str()])
            .current_dir(&self.location)
            .output()
            .await?;

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
        let file = File::open(paket_install.join("paket-installmodel.cache")).await.map_err(|e| {
            error!("unable to open file: {:?}", e);
            anyhow!(e)
        })?;
        let reader = BufReader::new(file);
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
                return Ok((path, smallest_framework.unwrap_or_default(), deps));
            }
        }

        Err(anyhow!("unable to get reference assembly"))
    }

    /// Load SDK XML files into the database.
    /// Processes all files in a single graph build and database write.
    /// Returns the count of successfully loaded files.
    ///
    /// The heavy work (filesystem access, tree-sitter parsing, SQLite writes)
    /// is offloaded to the blocking thread pool via `spawn_blocking`.
    #[instrument(skip_all, name = "deps.load_sdk_xml", fields(file_count = xml_files.len()))]
    pub async fn load_sdk_xml_files_to_database(
        &self,
        xml_files: Vec<PathBuf>,
    ) -> Result<usize, Error> {
        info!("Loading {} SDK XML files into database", xml_files.len());

        if xml_files.is_empty() {
            return Ok(0);
        }

        let db_path = self.db_path.clone();
        let span = tracing::Span::current();

        tokio::task::spawn_blocking(move || -> Result<usize, Error> {
            let _guard = span.enter();
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
            let (_, dep_source_type_node) = SourceType::load_symbols_into_graph(&mut graph);
            let graph = graph;

            // Create language configuration with all XML file analyzers
            let mut source_lc =
                SourceNodeLanguageConfiguration::new(&tree_sitter_stack_graphs::NoCancellation)?;

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
                    &source_lc.dependency_type_node_info,
                    &source_lc.language_config,
                    current_graph,
                ) {
                    std::result::Result::Ok(result) => {
                        success_count += result.files_loaded;
                        current_graph = result.stack_graph;
                        for (path, tag) in result.file_to_tag {
                            combined_file_to_tag.insert(path, tag);
                        }
                    }
                    Err(e) => {
                        error!("Failed to add SDK XML file {:?} to graph: {}", file, e);
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

            let root_node = root_node.ok_or_else(|| {
                error!("Unable to find root node in combined graph");
                anyhow!("unable to find root node")
            })?;

            let dep_source_type_node_handle = current_graph.iter_nodes().find(|n| {
                let node = &current_graph[*n];
                let symbol = node.symbol();
                symbol.is_some() && symbol.unwrap() == dep_source_type_node.get_symbol_handle()
            });

            if let Some(dep_node) = dep_source_type_node_handle {
                current_graph.add_edge(root_node, dep_node, 0);
            }

            // Write all results to database
            info!("Writing {} SDK XML files to database", success_count);
            let mut db: SQLiteWriter = SQLiteWriter::open(&db_path)?;

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
        })
        .await
        .map_err(|e| anyhow!("SDK XML loading task panicked: {}", e))?
    }

    /// Load SDK XML files from a given SDK path for a target framework
    /// This is a convenience method that finds and loads SDK XML files in one call
    pub(crate) async fn load_sdk_from_path(
        &self,
        sdk_path: &std::path::Path,
        target_framework: &crate::provider::target_framework::TargetFramework,
    ) -> Result<usize, Error> {
        info!(
            "Loading SDK from path {:?} for TFM {}",
            sdk_path,
            target_framework.as_str()
        );

        let xml_files =
            TargetFrameworkHelper::find_sdk_xml_files(&sdk_path.to_path_buf(), target_framework)?;

        if xml_files.is_empty() {
            info!("No SDK XML files found at {:?}", sdk_path);
            return Ok(0);
        }

        info!(
            "Found {} SDK XML files, loading into database",
            xml_files.len()
        );

        let result = self.load_sdk_xml_files_to_database(xml_files).await;

        // Store the SDK path for later use (e.g., notify_file_changes reload)
        if result.is_ok() {
            self.set_sdk_path(sdk_path.to_path_buf());
        }

        result
    }
}
