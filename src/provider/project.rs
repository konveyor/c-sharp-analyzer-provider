use std::{
    fmt::Debug,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Error};
use prost_types::{Struct, Value};
use serde::Deserialize;
use stack_graphs::{
    graph::StackGraph, serde::StackGraph as SerializableStackGraph,
    stitching::ForwardCandidates, storage::SQLiteReader, NoCancellation,
};
use tokio::sync::{Mutex as TokioMutex, RwLock};
use tracing::{debug, info, instrument, warn};
use crate::provider::telemetry::METRICS;
use which::which;

use crate::c_sharp_graph::language_config::SourceNodeLanguageConfiguration;
use crate::c_sharp_graph::loader::{init_stack_graph, SourceType};
use crate::provider::dependency_resolution::Dependencies;
use crate::provider::target_framework::TargetFramework;

pub struct Project {
    pub location: PathBuf,
    pub db_path: PathBuf,
    pub dependencies: Arc<TokioMutex<Option<Vec<Dependencies>>>>,
    pub graph: Arc<Mutex<Option<StackGraph>>>,
    pub source_language_config: Arc<RwLock<Option<SourceNodeLanguageConfiguration>>>,
    pub analysis_mode: AnalysisMode,
    pub tools: Tools,
    target_framework: Mutex<Option<TargetFramework>>,
    sdk_path: Mutex<Option<PathBuf>>,
}

#[derive(Eq, PartialEq, Debug, Deserialize)]
pub enum AnalysisMode {
    Full,
    SourceOnly,
}

impl From<&str> for AnalysisMode {
    fn from(value: &str) -> Self {
        match value {
            "full" => AnalysisMode::Full,
            "source-only" => AnalysisMode::SourceOnly,
            _ => AnalysisMode::Full,
        }
    }
}

impl From<&String> for AnalysisMode {
    fn from(value: &String) -> Self {
        AnalysisMode::from(value.as_str())
    }
}

impl From<String> for AnalysisMode {
    fn from(value: String) -> Self {
        AnalysisMode::from(value.as_str())
    }
}

impl Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("location", &self.location)
            .field("db_path", &self.db_path)
            .field("analysis_mode", &self.analysis_mode)
            .field("dependencies", &self.dependencies)
            .finish()
    }
}

#[derive(Clone)]
pub struct Tools {
    pub ilspy_cmd: PathBuf,
    pub paket_cmd: PathBuf,
    pub dotnet_install_cmd: Option<PathBuf>,
    pub dotnet_sdk_path: Option<PathBuf>,
}

impl Project {
    const ILSPY_CMD_LOC_KEY: &str = "ilspy_cmd";
    const PAKET_CMD_LOC_KEY: &str = "paket_cmd";
    const DOTNET_INSTALL_CMD_LOC_KEY: &str = "dotnet_install_cmd";
    const DOTNET_SDK_PATH_KEY: &str = "dotnet_sdk_path";
    const ILSPY_CMD: &str = "ilspy";
    const PAKET_CMD: &str = "paket";
    #[cfg(windows)]
    const DOTNET_INSTALL_SCRIPT: &str = "/usr/local/bin/scripts/dotnet-install.ps1";
    #[cfg(not(windows))]
    const DOTNET_INSTALL_SCRIPT: &str = "/usr/local/bin/scripts/dotnet-install.sh";
    pub fn new(
        location: PathBuf,
        db_path: PathBuf,
        analysis_mode: AnalysisMode,
        tools: Tools,
    ) -> Project {
        Project {
            location,
            db_path,
            dependencies: Arc::new(TokioMutex::new(None)),
            graph: Arc::new(Mutex::new(None)),
            source_language_config: Arc::new(RwLock::new(None)),
            analysis_mode,
            tools,
            target_framework: Mutex::new(None),
            sdk_path: Mutex::new(None),
        }
    }

    pub(crate) fn set_target_framework(&self, tfm: TargetFramework) {
        match self.target_framework.lock() {
            Ok(mut guard) => *guard = Some(tfm),
            Err(e) => tracing::warn!("Failed to set target framework: {}", e),
        }
    }

    pub(crate) fn set_sdk_path(&self, path: PathBuf) {
        if let Ok(mut guard) = self.sdk_path.lock() {
            *guard = Some(path);
        }
    }

    pub fn get_sdk_path(&self) -> Option<PathBuf> {
        // First check if we have an explicitly set SDK path
        if let Ok(guard) = self.sdk_path.lock() {
            if let Some(ref path) = *guard {
                return Some(path.clone());
            }
        }
        // Fall back to deriving from target framework (for backward compatibility)
        match self.target_framework.lock() {
            Ok(guard) => {
                if let Some(ref tfm) = *guard {
                    return Some(std::env::temp_dir().join("dotnet-sdks").join(tfm.as_str()));
                }
            }
            Err(e) => tracing::warn!("Failed to read target framework: {}", e),
        }
        None
    }

    pub fn get_tools(specific_provider_config: &Option<Struct>) -> Result<Tools, Error> {
        match specific_provider_config {
            Some(specific_provider_config) => {
                let ilspy_cmd = match specific_provider_config.fields.get(Self::ILSPY_CMD_LOC_KEY) {
                    Some(Value {
                        kind: Some(prost_types::value::Kind::StringValue(s)),
                    }) => {
                        let p = PathBuf::from_str(s)?;
                        if p.exists() {
                            p
                        } else {
                            return Err(anyhow!("not valid ilspycmd"));
                        }
                    }
                    None => which(Self::ILSPY_CMD)?,
                    _ => {
                        return Err(anyhow!("not valid ilspycmd"));
                    }
                };
                let paket_cmd = match specific_provider_config.fields.get(Self::PAKET_CMD_LOC_KEY) {
                    Some(Value {
                        kind: Some(prost_types::value::Kind::StringValue(s)),
                    }) => {
                        let p = PathBuf::from_str(s)?;
                        if p.exists() {
                            p
                        } else {
                            return Err(anyhow!("not valid paket_cmd"));
                        }
                    }
                    None => which::which(Self::PAKET_CMD)?,
                    _ => {
                        return Err(anyhow!("not valid paket_cmd"));
                    }
                };
                let dotnet_install_cmd = match specific_provider_config
                    .fields
                    .get(Self::DOTNET_INSTALL_CMD_LOC_KEY)
                {
                    Some(Value {
                        kind: Some(prost_types::value::Kind::StringValue(s)),
                    }) => {
                        let p = PathBuf::from_str(s)?;
                        if p.exists() {
                            Some(p)
                        } else {
                            warn!("Configured dotnet_install_cmd not found at {}, SDK installation will be unavailable", p.display());
                            None
                        }
                    }
                    None => {
                        let default_path = PathBuf::from(Self::DOTNET_INSTALL_SCRIPT);
                        if default_path.exists() {
                            Some(default_path)
                        } else {
                            warn!("dotnet-install script not found at {}, SDK installation will be unavailable", Self::DOTNET_INSTALL_SCRIPT);
                            None
                        }
                    }
                    _ => {
                        warn!("Invalid dotnet_install_cmd configuration, SDK installation will be unavailable");
                        None
                    }
                };
                let dotnet_sdk_path = match specific_provider_config
                    .fields
                    .get(Self::DOTNET_SDK_PATH_KEY)
                {
                    Some(Value {
                        kind: Some(prost_types::value::Kind::StringValue(s)),
                    }) => {
                        let p = PathBuf::from_str(s)?;
                        if p.exists() {
                            info!("Using configured .NET SDK path: {:?}", p);
                            Some(p)
                        } else {
                            warn!(
                                "Configured dotnet_sdk_path {} does not exist, will try auto-detection",
                                p.display()
                            );
                            None
                        }
                    }
                    None => None,
                    _ => {
                        warn!("Invalid dotnet_sdk_path configuration");
                        None
                    }
                };
                Ok(Tools {
                    ilspy_cmd,
                    paket_cmd,
                    dotnet_install_cmd,
                    dotnet_sdk_path,
                })
            }
            None => {
                let default_path = PathBuf::from(Self::DOTNET_INSTALL_SCRIPT);
                let dotnet_install_cmd = if default_path.exists() {
                    Some(default_path)
                } else {
                    warn!("dotnet-install script not found at {}, SDK installation will be unavailable", Self::DOTNET_INSTALL_SCRIPT);
                    None
                };
                Ok(Tools {
                    ilspy_cmd: which(Self::ILSPY_CMD)?,
                    paket_cmd: which(Self::PAKET_CMD)?,
                    dotnet_install_cmd,
                    dotnet_sdk_path: None,
                })
            }
        }
    }

    pub async fn validate_language_configuration(&self) -> Result<(), Error> {
        let lc = SourceNodeLanguageConfiguration::new(&tree_sitter_stack_graphs::NoCancellation)?;
        let mut lc_guard = self.source_language_config.write().await;
        lc_guard.replace(lc);
        Ok(())
    }

    #[instrument(skip_all, name = "project.get_graph")]
    pub async fn get_project_graph(&self) -> Result<usize, Error> {
        if self.db_path.exists() {
            debug!("trying to load from existing db: {:?}", &self.db_path);

            // Run blocking SQLite + graph deserialization on the blocking pool
            let db_path = self.db_path.clone();
            let location = self.location.clone();
            let graph_arc = self.graph.clone();
            let sdk_path = self.get_sdk_path();

            let span = tracing::info_span!("project.load_from_db");
            let result = tokio::task::spawn_blocking(move || -> Result<Option<usize>, Error> {
                let _guard = span.enter();
                let mut db_reader = SQLiteReader::open(&db_path).map_err(|e| anyhow!(e))?;
                db_reader
                    .load_graphs_for_file_or_directory(&location, &NoCancellation)?;

                // Also load SDK XML files if target framework is set
                if let Some(sdk_path) = sdk_path {
                    debug!("Loading SDK graphs from: {:?}", sdk_path);
                    let _ = db_reader
                        .load_graphs_for_file_or_directory(&sdk_path, &NoCancellation);
                }

                let (stack_graph, _, _) = db_reader.get_graph_partials_and_db();
                let file_count = stack_graph.iter_files().count();
                debug!("got stack graph from db with {} files", file_count);

                let serializable_graph = SerializableStackGraph::from_graph(stack_graph);
                let mut graph = StackGraph::new();
                if let Err(e) = serializable_graph.load_into(&mut graph) {
                    debug!("unable to load graph: {}", e);
                }

                if graph.iter_symbols().count() == 0 {
                    debug!("unable to load graph from db, will build from source");
                    return Ok(None);
                }

                let mut graph_guard = graph_arc.lock()
                    .unwrap_or_else(|e| e.into_inner());
                graph_guard.replace(graph);
                debug!("loaded graph from existing db");
                Ok(Some(file_count))
            })
            .await
            .map_err(|e| anyhow!("db loading task panicked: {}", e))??;

            if let Some(file_count) = result {
                METRICS.files_indexed.set(file_count as i64);
                return Ok(file_count);
            }
        }

        // No existing DB or it was empty -- build from source.
        // Acquire an owned read guard so it can be sent to spawn_blocking.
        let lc_guard = Arc::clone(&self.source_language_config).read_owned().await;
        let graph_arc = self.graph.clone();
        let location = self.location.clone();
        let db_path = self.db_path.clone();

        let span = tracing::info_span!("project.build_from_source");
        let files_loaded = tokio::task::spawn_blocking(move || -> Result<usize, Error> {
            let _guard = span.enter();
            let lc = lc_guard.as_ref()
                .ok_or_else(|| anyhow::anyhow!("language configuration not initialized"))?;

            let initialized_results = init_stack_graph(
                &location,
                &db_path,
                &lc.source_type_node_info,
                &lc.language_config,
            ).map_err(|e| anyhow!(e))?;

            let mut graph_guard = graph_arc.lock()
                .unwrap_or_else(|e| e.into_inner());
            graph_guard.replace(initialized_results.stack_graph);
            METRICS.files_indexed.set(initialized_results.files_loaded as i64);
            Ok(initialized_results.files_loaded)
        })
        .await
        .map_err(|e| anyhow!("init_stack_graph task panicked: {}", e))??;

        Ok(files_loaded)
    }

    #[instrument(skip_all, name = "project.get_source_type")]
    pub async fn get_source_type(&self) -> Option<Arc<SourceType>> {
        let lc_guard = self.source_language_config.read().await;

        match lc_guard.as_ref() {
            Some(x) => match self.analysis_mode {
                AnalysisMode::SourceOnly => Some(x.source_type_node_info.clone()),
                AnalysisMode::Full => Some(x.dependency_type_node_info.clone()),
            },
            None => None,
        }
    }
}
