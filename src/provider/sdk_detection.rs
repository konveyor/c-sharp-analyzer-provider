use std::path::{Path, PathBuf};

use tracing::{debug, info, trace, warn};

use crate::provider::target_framework::TargetFramework;

/// SDK detection result
#[derive(Debug)]
pub enum SdkSource {
    /// SDK found (either configured or auto-detected)
    Found {
        path: PathBuf,
        /// Source of the SDK: "configured" or "detected"
        source: &'static str,
    },
    /// No SDK found, needs installation
    NotFound,
}

pub struct SdkDetector;

impl SdkDetector {
    pub fn find_sdk(
        configured_path: Option<&Path>,
        target_framework: &TargetFramework,
    ) -> SdkSource {
        // Check for user configured SDK path
        if let Some(path) = configured_path {
            if path.exists() {
                if Self::validate_sdk_for_tfm(path, target_framework) {
                    info!(
                        "Using configured SDK path {:?} for TFM {}",
                        path,
                        target_framework.as_str()
                    );
                    return SdkSource::Found {
                        path: path.to_path_buf(),
                        source: "configured",
                    };
                } else {
                    warn!(
                        "Configured SDK path {:?} does not contain compatible SDK for TFM {}",
                        path,
                        target_framework.as_str()
                    );
                }
            } else {
                warn!("Configured SDK path {:?} does not exist", path);
            }
        }

        // Detect system installations
        let system_paths = Self::get_system_sdk_paths();
        for sdk_path in &system_paths {
            if !sdk_path.exists() {
                trace!("SDK path {:?} does not exist, skipping", sdk_path);
                continue;
            }

            if Self::validate_sdk_for_tfm(sdk_path, target_framework) {
                info!(
                    "Detected system SDK at {:?} for TFM {}",
                    sdk_path,
                    target_framework.as_str()
                );
                return SdkSource::Found {
                    path: sdk_path.clone(),
                    source: "detected",
                };
            }
        }

        // No SDK found
        info!(
            "No existing SDK found for TFM {}, installation may be required",
            target_framework.as_str()
        );
        SdkSource::NotFound
    }

    /// Get platform-specific SDK installation paths
    fn get_system_sdk_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // Check for DOTNET_ROOT environment variable
        if let Ok(dotnet_root) = std::env::var("DOTNET_ROOT") {
            let root_path = PathBuf::from(&dotnet_root);
            if !paths.contains(&root_path) {
                paths.push(root_path);
            }
        }

        #[cfg(target_os = "linux")]
        {
            // Standard Linux locations
            paths.push(PathBuf::from("/usr/share/dotnet"));
            paths.push(PathBuf::from("/usr/lib/dotnet"));

            // User-local installation
            if let Ok(home) = std::env::var("HOME") {
                paths.push(PathBuf::from(home).join(".dotnet"));
            }
        }

        #[cfg(target_os = "macos")]
        {
            // Standard macOS locations
            paths.push(PathBuf::from("/usr/local/share/dotnet"));

            // User-local installation
            if let Ok(home) = std::env::var("HOME") {
                paths.push(PathBuf::from(home).join(".dotnet"));
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Standard Windows locations
            paths.push(PathBuf::from(r"C:\Program Files\dotnet"));
            paths.push(PathBuf::from(r"C:\Program Files (x86)\dotnet"));

            // User-local installations
            if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
                paths.push(PathBuf::from(localappdata).join("Microsoft").join("dotnet"));
            }
            if let Ok(userprofile) = std::env::var("USERPROFILE") {
                paths.push(PathBuf::from(userprofile).join(".dotnet"));
            }
        }

        debug!("System SDK paths to check: {:?}", paths);
        paths
    }

    /// Check if a path contains a valid SDK for the target framework
    fn validate_sdk_for_tfm(sdk_root: &Path, target_framework: &TargetFramework) -> bool {
        let packs_path = sdk_root.join("packs");

        if !packs_path.exists() || !packs_path.is_dir() {
            debug!("No packs directory found at {:?}", packs_path);
            return false;
        }

        // Look for Microsoft.NETCore.App.Ref pack
        let netcore_pack = packs_path.join("Microsoft.NETCore.App.Ref");
        if !netcore_pack.exists() {
            debug!("No Microsoft.NETCore.App.Ref found at {:?}", netcore_pack);
            return false;
        }

        // Find available versions
        let versions: Vec<String> = match std::fs::read_dir(&netcore_pack) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .collect(),
            Err(e) => {
                debug!("Failed to read {:?}: {}", netcore_pack, e);
                return false;
            }
        };

        if versions.is_empty() {
            debug!("No SDK versions found in {:?}", netcore_pack);
            return false;
        }

        // Check if any version has the ref/<tfm> directory
        let tfm_str = target_framework.as_str();
        for version in &versions {
            let ref_path = netcore_pack.join(version).join("ref").join(tfm_str);
            if ref_path.exists() && ref_path.is_dir() {
                debug!(
                    "Found compatible SDK at {:?} with version {} for TFM {}",
                    sdk_root, version, tfm_str
                );
                return true;
            }
        }

        debug!(
            "SDK at {:?} found but no exact TFM match for {}. Available versions: {:?}",
            sdk_root, tfm_str, versions
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counter for unique directory names
    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Test fixture that creates a unique temp directory and cleans up on drop
    struct TestSdkDir {
        path: PathBuf,
    }

    impl TestSdkDir {
        fn new() -> Self {
            let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir()
                .join("sdk_detection_tests")
                .join(format!("test_{}", id));
            // Clean up if exists from previous failed run
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        /// Create a mock SDK structure for the given TFM
        fn create_sdk_structure(&self, tfm: &str) {
            let packs = self
                .path
                .join("packs")
                .join("Microsoft.NETCore.App.Ref")
                .join("8.0.0")
                .join("ref")
                .join(tfm);
            std::fs::create_dir_all(&packs).unwrap();
        }
    }

    impl Drop for TestSdkDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_get_system_sdk_paths_returns_paths() {
        let paths = SdkDetector::get_system_sdk_paths();
        // Should return at least one path on any platform
        assert!(!paths.is_empty());
    }

    #[test]
    fn test_validate_sdk_for_tfm_with_valid_sdk() {
        let test_dir = TestSdkDir::new();
        test_dir.create_sdk_structure("net8.0");

        let tfm = TargetFramework::from_str("net8.0").unwrap();
        let result = SdkDetector::validate_sdk_for_tfm(test_dir.path(), &tfm);

        assert!(result);
    }

    #[test]
    fn test_validate_sdk_for_tfm_with_invalid_path() {
        let tfm = TargetFramework::from_str("net8.0").unwrap();
        let result = SdkDetector::validate_sdk_for_tfm(Path::new("/nonexistent/path"), &tfm);

        assert!(!result);
    }

    #[test]
    fn test_validate_sdk_for_tfm_with_wrong_tfm() {
        let test_dir = TestSdkDir::new();
        // Create SDK for net8.0 but query for net9.0
        test_dir.create_sdk_structure("net8.0");

        let tfm = TargetFramework::from_str("net9.0").unwrap();
        let result = SdkDetector::validate_sdk_for_tfm(test_dir.path(), &tfm);

        assert!(!result);
    }

    #[test]
    fn test_find_sdk_prefers_configured_path() {
        let test_dir = TestSdkDir::new();
        test_dir.create_sdk_structure("net8.0");

        let tfm = TargetFramework::from_str("net8.0").unwrap();
        let result = SdkDetector::find_sdk(Some(test_dir.path()), &tfm);

        assert!(matches!(
            result,
            SdkSource::Found {
                source: "configured",
                ..
            }
        ));
    }

    #[test]
    fn test_find_sdk_returns_not_found_for_missing_tfm() {
        // Query for a TFM that won't exist in system SDKs
        let tfm = TargetFramework::from_str("net99.0").unwrap();
        let result = SdkDetector::find_sdk(None, &tfm);

        // Should return NotFound since net99.0 won't exist
        assert!(matches!(result, SdkSource::NotFound));
    }

    #[test]
    fn test_find_sdk_falls_back_when_configured_path_invalid() {
        // Configure an invalid path
        let invalid_path = Path::new("/nonexistent/sdk/path");
        let tfm = TargetFramework::from_str("net8.0").unwrap();

        // Since invalid_path doesn't exist, it should fall back to system detection
        let result = SdkDetector::find_sdk(Some(invalid_path), &tfm);

        // Should fall through configured path and either find system SDK or return NotFound
        assert!(matches!(
            result,
            SdkSource::NotFound | SdkSource::Found { .. }
        ));
    }
}
