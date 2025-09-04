use anyhow::{anyhow, Error};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::task::JoinSet;
use tracing::{debug, error, info, trace};

use crate::c_sharp_graph::loader::load_database;

const REFERNCE_ASSEMBLIES_NAME: &str = "Microsoft.NETFramework.ReferenceAssemblies";

#[derive(Debug)]
pub struct Dependencies {
    pub location: PathBuf,
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub version: String,
    pub decompiled_location: Arc<Mutex<HashSet<PathBuf>>>,
}

#[derive(Debug)]
pub struct Project {
    pub location: String,
    pub dependencies: Arc<Mutex<Option<Vec<Dependencies>>>>,
}

impl Project {
    pub fn new(location: String) -> Arc<Project> {
        Arc::new(Project {
            location,
            dependencies: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn resolve(self: &Arc<Self>) -> Result<(), Error> {
        // First need to run packet.
        // Need to convert and download all DLL's
        //TODO: Add paket location as a provider specific config.
        let paket_output = Command::new("/Users/shurley/.dotnet/tools/paket")
            .args(["convert-from-nuget", "-f"])
            .current_dir(self.location.as_str())
            .output()?;

        let deps_response = self.read_packet_output(paket_output);
        let deps = match deps_response {
            Ok(d) => d,
            Err(e) => {
                return Err(e);
            }
        };
        let paket_deps_file = PathBuf::from(self.location.clone()).join("paket.dependencies");
        let (reference_assembly_path, highest_restriction) = self
            .get_reference_assemblies(paket_deps_file.as_path())
            .await?;
        debug!(
            "got: {:?} -- {:?}",
            reference_assembly_path, highest_restriction
        );
        let mut set = JoinSet::new();
        for d in deps {
            let reference_assmblies = reference_assembly_path.clone();
            let restriction = highest_restriction.clone();
            set.spawn(async move {
                let decomp = d.decompile(reference_assmblies, restriction).await;
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
                Ok(d) => {
                    deps.push(d);
                }
                Err(e) => {
                    return Err(Error::new(e));
                }
            }
        }
        let mut d = self.dependencies.lock().unwrap();
        *d = Some(deps);

        Ok(())
    }

    fn read_packet_output(&self, output: Output) -> Result<Vec<Dependencies>, Error> {
        if !output.status.success() {
            //TODO: Consider a specific error type
            debug!("paket command not successful");
            return Err(Error::msg("paket command did not succeed"));
        }
        // We need to get the Reference Assemblies after we successfully
        // convert to paket.
        // Either this will be input into the init
        // Or we will find a clever way to get it from the .csproj file
        // For speed going to hardcoded for now.
        let lines = String::from_utf8_lossy(&output.stdout).to_string();
        let path = PathBuf::from(&self.location);
        // Exampale lines to parse:
        // - Microsoft.SqlServer.Types is pinned to 10.50.1600.1
        // - Newtonsoft.Json is pinned to 5.0.4
        // - EntityFramework is pinned to 5.0.0
        // - DotNetOpenAuth.AspNet is pinned to 4.3.0.13117
        let mut deps: Vec<Dependencies> = vec![];
        for line in lines.lines() {
            if !line.contains("-") || !line.contains("is pinned to") {
                continue;
            }

            let parts: Vec<&str> = line.split("is pinned to").collect();

            // Example parts
            // [\" - DotNetOpenAuth.OpenId.Core \", \" 4.3.0.13117\"]"
            let name = match parts[0].trim().strip_prefix("- ") {
                Some(n) => n,
                None => parts[0],
            };
            let version = parts[1].trim();
            let mut dep_path = path.clone().to_path_buf();
            dep_path.push("packages");
            dep_path.push(name);

            let d = Dependencies {
                location: dep_path,
                name: name.to_string(),
                version: version.to_string(),
                decompiled_location: Arc::new(Mutex::new(HashSet::new())),
            };
            deps.push(d);
        }
        Ok(deps)
    }

    async fn get_reference_assemblies(
        &self,
        paket_deps_file: &Path,
    ) -> Result<(PathBuf, String), Error> {
        let file = File::open(paket_deps_file).await;
        if let Err(e) = file {
            error!("unable to find error: {:?}", e);
            return Err(anyhow!(e));
        }
        let reader = BufReader::new(file.ok().unwrap());
        let mut lines = reader.lines();
        let mut smallest_framework = "zzzzzzzzzzzzzzz".to_string();
        while let Some(line) = lines.next_line().await? {
            if !line.contains("restriction") {
                continue;
            }
            let parts: Vec<&str> = line.split("restriction:").collect();
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

        // Now we we have the framework, we need to get the reference_assmblies
        let base_name = format!("{}.{}", REFERNCE_ASSEMBLIES_NAME, smallest_framework);
        let paket_reference_output = Command::new("/Users/shurley/.dotnet/tools/paket")
            .args(["add", base_name.as_str()])
            .current_dir(self.location.as_str())
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
            if line.contains("build/.NETFramework/")
                && line.contains("D:")
                && let Some(path_str) = line.strip_prefix("D: /")
            {
                debug!("path_str: {}", path_str);
                let path = paket_install.join(path_str);
                return Ok((paket_install.join(path), smallest_framework));
            }
        }

        Err(anyhow!("unable to get reference assembly"))
    }

    pub async fn load_to_database(&self, db_path: PathBuf) -> Result<(), Error> {
        let db = Arc::new(db_path);
        let shared_deps = Arc::clone(&self.dependencies);
        let mut x = shared_deps.lock().unwrap();
        if let Some(ref mut vec) = *x {
            // For each dependnecy in the list we will try and load the decompiled files
            // Into the stack graph database.
            for d in vec {
                let decompiled_locations: Arc<Mutex<HashSet<PathBuf>>> =
                    Arc::clone(&d.decompiled_location);
                let decompiled_locations = decompiled_locations.lock().unwrap();
                let decompiled_files = &(*decompiled_locations);
                for decompiled_file in decompiled_files {
                    debug!("loading file {:?} into database", &decompiled_file);
                    let stats = load_database(decompiled_file, db.to_path_buf());
                    debug!(
                        "loaded file: {:?} for dep: {:?} stats: {:?}",
                        &decompiled_file, d, stats
                    );
                }
            }
        }
        Ok(())
    }
}

impl Dependencies {
    pub async fn decompile(
        &self,
        reference_assmblies: PathBuf,
        restriction: String,
    ) -> Result<(), Error> {
        // TODO: make location of ilspycmd decompilation
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
                debug!("did not find a dll for dep: {:?}", self);
                return Err(anyhow!("unable to find dll's"));
            }
        };
        let mut decompiled_files: HashSet<PathBuf> = HashSet::new();
        for file_to_decompile in to_decompile_locations {
            let decompiled_file = self
                .decompile_file(&reference_assmblies, file_to_decompile)
                .await?;
            decompiled_files.insert(decompiled_file);
        }

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
        info!("Reading packet cache file: {:?}", file);
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

        if dlls.is_empty() {
            error!("Unable to get dlls from file");
        }
        Ok(dll_paths)
    }

    async fn decompile_file(
        &self,
        reference_assmblies: &PathBuf,
        file_to_decompile: PathBuf,
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
        let decompile_output = Command::new("/Users/shurley/.dotnet/tools/ilspycmd")
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
}
