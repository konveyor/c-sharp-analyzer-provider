use anyhow::{anyhow, Error};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, error, info};
use walkdir::WalkDir;

/// Represents a Target Framework Moniker (TFM)
/// See: https://learn.microsoft.com/en-us/dotnet/standard/frameworks
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TargetFramework(String);

impl fmt::Display for TargetFramework {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TargetFramework {
    /// Parse TFM from a .csproj file
    /// Supports both new SDK-style and old .NET Framework .csproj formats
    /// Handles all TFM formats from Microsoft documentation
    pub(crate) fn from_csproj(csproj_path: &PathBuf) -> Result<Self, Error> {
        debug!("Parsing TargetFramework from {:?}", csproj_path);

        let file = File::open(csproj_path)
            .map_err(|e| anyhow!("Failed to open .csproj file {:?}: {}", csproj_path, e))?;
        let buf_reader = BufReader::new(file);
        let mut reader = Reader::from_reader(buf_reader);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut in_property_group = false;
        let mut target_framework: Option<String> = None;
        let mut target_framework_version: Option<String> = None;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let name = e.name();
                    match name.as_ref() {
                        b"PropertyGroup" => {
                            in_property_group = true;
                        }
                        b"TargetFramework" if in_property_group => {
                            // New-style .NET Core/.NET 5+ projects use <TargetFramework>
                            if let Ok(Event::Text(text)) = reader.read_event_into(&mut buf) {
                                let value = String::from_utf8_lossy(&text).trim().to_string();
                                if !value.is_empty() {
                                    target_framework = Some(value);
                                }
                            }
                        }
                        b"TargetFrameworkVersion" if in_property_group => {
                            // Old-style .NET Framework projects use <TargetFrameworkVersion>
                            if let Ok(Event::Text(text)) = reader.read_event_into(&mut buf) {
                                let value = String::from_utf8_lossy(&text).trim().to_string();
                                if !value.is_empty() {
                                    target_framework_version = Some(value);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    if e.name().as_ref() == b"PropertyGroup" {
                        in_property_group = false;
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(anyhow!(
                        "XML parsing error at position {}: {}",
                        reader.buffer_position(),
                        e
                    ));
                }
                _ => {}
            }
            buf.clear();
        }

        // Prefer TargetFramework over TargetFrameworkVersion
        let framework = target_framework
            .or(target_framework_version)
            .ok_or_else(|| {
                anyhow!(
                    "No TargetFramework or TargetFrameworkVersion found in {:?}",
                    csproj_path
                )
            })?;

        // Normalize the framework string
        let normalized = Self::normalize(&framework)?;

        debug!("TargetFramework for {:?}: {}", csproj_path, normalized);
        Ok(TargetFramework(normalized))
    }

    /// Create a TargetFramework from a string, normalizing it
    #[allow(dead_code)]
    pub(crate) fn from_str(tfm: &str) -> Result<Self, Error> {
        let normalized = Self::normalize(tfm)?;
        Ok(TargetFramework(normalized))
    }

    /// Get the underlying TFM string
    #[allow(dead_code)]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert TFM to dotnet-install channel format
    /// The dotnet-install script expects version numbers, not TFM format
    ///
    /// Examples:
    /// - "net9.0" -> "9.0"
    /// - "net8.0" -> "8.0"
    /// - "net6.0" -> "6.0"
    /// - "netcoreapp3.1" -> "3.1"
    /// - "net48" -> "4.8" (for .NET Framework, though install script may not support)
    pub(crate) fn to_channel(&self) -> Result<String, Error> {
        let tfm = &self.0;

        // Modern .NET (net5.0+)
        if tfm.starts_with("net")
            && tfm.contains('.')
            && !tfm.starts_with("netcoreapp")
            && !tfm.starts_with("netstandard")
        {
            let version = &tfm[3..]; // Strip "net" prefix
            return Ok(version.to_string());
        }

        // .NET Core (netcoreapp3.1, etc.)
        if let Some(version) = tfm.strip_prefix("netcoreapp") {
            return Ok(version.to_string());
        }

        // .NET Standard (netstandard2.1, etc.)
        // Note: dotnet-install doesn't install .NET Standard, but we return the version anyway
        if let Some(version) = tfm.strip_prefix("netstandard") {
            return Ok(version.to_string());
        }

        // .NET Framework (net48, net472, etc.)
        // Convert net48 -> 4.8, net472 -> 4.7.2
        if tfm.starts_with("net") && !tfm.contains('.') {
            let version_part = &tfm[3..];
            if version_part.len() == 2 {
                // net48 -> 4.8
                let major = &version_part[0..1];
                let minor = &version_part[1..2];
                return Ok(format!("{}.{}", major, minor));
            } else if version_part.len() == 3 {
                // net472 -> 4.7.2
                let major = &version_part[0..1];
                let minor1 = &version_part[1..2];
                let minor2 = &version_part[2..3];
                return Ok(format!("{}.{}.{}", major, minor1, minor2));
            }
        }

        Err(anyhow!(
            "Cannot convert TFM '{}' to dotnet-install channel",
            tfm
        ))
    }

    /// Install the .NET SDK for this target framework
    /// Returns the path to the installed SDK directory
    pub(crate) fn install_sdk(&self, dotnet_install_script: &PathBuf) -> Result<PathBuf, Error> {
        info!("install_sdk() called for target framework: {}", self.0);
        info!("Script path: {:?}, exists: {}", dotnet_install_script, dotnet_install_script.exists());

        // Convert TFM to channel format for dotnet-install script
        let channel = self.to_channel()?;
        info!("Converted TFM {} to channel {}", self.0, channel);

        // Determine the installation directory
        let install_dir = std::env::temp_dir().join("dotnet-sdks").join(&self.0);
        info!("Target installation directory: {:?}", install_dir);
        std::fs::create_dir_all(&install_dir)?;
        info!("Created/verified installation directory exists");

        info!(
            "Executing dotnet-install script at: {:?}",
            dotnet_install_script
        );

        // Run the installation script
        info!("Running dotnet-install for channel {} to {:?}", channel, install_dir);
        let output = if cfg!(windows) {
            Command::new("powershell")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-File")
                .arg(dotnet_install_script)
                .arg("-InstallDir")
                .arg(&install_dir)
                .arg("-Channel")
                .arg(&channel)
                .output()?
        } else {
            Command::new(dotnet_install_script)
                .arg("--install-dir")
                .arg(&install_dir)
                .arg("--channel")
                .arg(&channel)
                .output()?
        };

        info!("dotnet-install script completed with status: {:?}", output.status);
        info!("Script stdout: {}", String::from_utf8_lossy(&output.stdout));
        info!("Script stderr: {}", String::from_utf8_lossy(&output.stderr));

        if !output.status.success() {
            error!(
                "dotnet-install script failed with status {:?}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            return Err(anyhow!("Failed to install .NET SDK for {}: {}", self.0, String::from_utf8_lossy(&output.stderr)));
        }

        info!("Successfully installed .NET SDK to {:?}", install_dir);
        Ok(install_dir)
    }

    /// Normalize a Target Framework Moniker (TFM) to a standard format
    /// Handles all TFM formats from https://learn.microsoft.com/en-us/dotnet/standard/frameworks
    ///
    /// Examples:
    /// - "v4.5" -> "net45" (old .NET Framework format)
    /// - "net8.0" -> "net8.0" (modern .NET)
    /// - "net8.0-android" -> "net8.0" (strip platform suffix for SDK installation)
    /// - "net8.0-ios17.2" -> "net8.0" (strip platform and version for SDK installation)
    /// - "netstandard2.1" -> "netstandard2.1" (pass through)
    /// - "netcoreapp3.1" -> "netcoreapp3.1" (pass through)
    fn normalize(framework: &str) -> Result<String, Error> {
        let trimmed = framework.trim();

        // Handle old .NET Framework versions (v4.5, v4.7.2, etc.)
        if trimmed.starts_with('v') {
            // Convert v4.5 -> net45, v4.7.2 -> net472
            let version_part = trimmed.trim_start_matches('v').replace('.', "");
            return Ok(format!("net{}", version_part));
        }

        // Handle platform-specific TFMs by stripping the platform suffix
        // Examples: net8.0-android, net8.0-ios17.2, net8.0-windows10.0.19041
        // We only need the base framework for SDK installation
        if let Some(dash_pos) = trimmed.find('-') {
            let base_framework = &trimmed[..dash_pos];

            // Validate that the base part is a valid TFM
            if Self::is_valid_base_tfm(base_framework) {
                debug!(
                    "Stripping platform suffix from TFM: {} -> {}",
                    trimmed, base_framework
                );
                return Ok(base_framework.to_string());
            } else {
                return Err(anyhow!(
                    "Invalid base TFM '{}' in platform-specific TFM '{}'",
                    base_framework,
                    trimmed
                ));
            }
        }

        // Validate and pass through modern TFMs
        // Valid formats: netX.Y (net5.0+), netstandard, netcoreapp, netXX (net45, net472)
        if Self::is_valid_base_tfm(trimmed) {
            Ok(trimmed.to_string())
        } else {
            Err(anyhow!(
                "Unrecognized Target Framework Moniker: {}",
                trimmed
            ))
        }
    }

    /// Check if a TFM base name is valid
    /// Valid formats include:
    /// - netX.Y (e.g., net5.0, net6.0, net7.0, net8.0, net9.0, net10.0)
    /// - netXX (e.g., net45, net451, net46, net461, net462, net47, net471, net472, net48, net481)
    /// - netstandardX.Y (e.g., netstandard1.0 through netstandard2.1)
    /// - netcoreappX.Y (e.g., netcoreapp1.0, netcoreapp2.0, netcoreapp3.0, netcoreapp3.1)
    ///
    /// Note: Platform-specific TFMs (e.g., net8.0-android) are NOT valid base TFMs
    fn is_valid_base_tfm(tfm: &str) -> bool {
        // Platform-specific TFMs should not be accepted as base TFMs
        if tfm.contains('-') {
            return false;
        }

        // Check for .NET Standard (netstandard1.0 - netstandard2.1)
        // This must come before the generic "net" check
        if let Some(version_part) = tfm.strip_prefix("netstandard") {
            // Should be in format X.Y where X is 1 or 2
            if let Some(dot_pos) = version_part.find('.') {
                if let Ok(major) = version_part[..dot_pos].parse::<u32>() {
                    return (1..=2).contains(&major);
                }
            }
            return false;
        }

        // Check for .NET Core (netcoreapp1.0, netcoreapp2.0, netcoreapp2.1, netcoreapp3.0, netcoreapp3.1)
        // This must come before the generic "net" check
        if let Some(version_part) = tfm.strip_prefix("netcoreapp") {
            // Should be in format X.Y where X is 1, 2, or 3
            if let Some(dot_pos) = version_part.find('.') {
                if let Ok(major) = version_part[..dot_pos].parse::<u32>() {
                    return (1..=3).contains(&major);
                }
            }
            return false;
        }

        // Check for modern .NET (net5.0+)
        if tfm.starts_with("net") && tfm.contains('.') {
            let version_part = &tfm[3..];
            // Should be in format X.Y where X is a number >= 5
            if let Some(dot_pos) = version_part.find('.') {
                if let Ok(major) = version_part[..dot_pos].parse::<u32>() {
                    return major >= 5;
                }
            }
            return false;
        }

        // Check for .NET Framework (net45, net451, net46, net461, net462, net47, net471, net472, net48, net481)
        if tfm.starts_with("net") && !tfm.contains('.') {
            let version_part = &tfm[3..];
            // Should be 2-3 digits (45, 451, 46, 461, 462, 47, 471, 472, 48, 481)
            if version_part.len() >= 2 && version_part.len() <= 3 {
                return version_part.chars().all(|c| c.is_ascii_digit());
            }
            return false;
        }

        false
    }
}

/// Helper functions for working with multiple TFMs
pub(crate) struct TargetFrameworkHelper;

impl TargetFrameworkHelper {
    /// Find all .csproj files in a directory and subdirectories
    pub(crate) fn find_csproj_files(location: &PathBuf) -> Result<Vec<PathBuf>, Error> {
        let mut csproj_files = Vec::new();

        for entry in WalkDir::new(location)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                if let Some(extension) = entry.path().extension() {
                    if extension == "csproj" {
                        csproj_files.push(entry.path().to_path_buf());
                    }
                }
            }
        }

        info!(
            "Found {} .csproj files in {:?}",
            csproj_files.len(),
            location
        );
        Ok(csproj_files)
    }

    /// Get the earliest target framework version from all .csproj files in a directory
    pub(crate) fn get_earliest_from_directory(
        location: &PathBuf,
    ) -> Result<TargetFramework, Error> {
        let csproj_files = Self::find_csproj_files(location)?;

        if csproj_files.is_empty() {
            return Err(anyhow!("No .csproj files found in {:?}", location));
        }

        let mut frameworks = Vec::new();
        for csproj in &csproj_files {
            match TargetFramework::from_csproj(csproj) {
                Ok(framework) => frameworks.push(framework),
                Err(e) => {
                    debug!("Failed to get target framework for {:?}: {}", csproj, e);
                    // Continue processing other projects
                }
            }
        }

        if frameworks.is_empty() {
            return Err(anyhow!(
                "Could not determine target framework from any .csproj file"
            ));
        }

        // Sort to get the earliest version (lexicographically smallest)
        frameworks.sort();
        let earliest = frameworks.into_iter().next().unwrap();

        info!("Earliest target framework detected: {}", earliest.as_str());
        Ok(earliest)
    }

    /// Find and collect XML documentation files from the SDK reference packs
    /// Looks in SDK_PATH/packs/ for:
    /// - Microsoft.NETCore.App.Ref/<version>/ref/<tfm>
    /// - Microsoft.AspNetCore.App.Ref/<version>/ref/<tfm>
    ///
    /// Skips OS-specific packs and NetStandard directories
    pub(crate) fn find_sdk_xml_files(
        sdk_path: &PathBuf,
        tfm: &TargetFramework,
    ) -> Result<Vec<PathBuf>, Error> {
        let mut xml_files = Vec::new();

        // Reference packs are in sdk_path/packs/
        let packs_dir = sdk_path.join("packs");

        if !packs_dir.exists() {
            debug!("Packs directory not found at {:?}", packs_dir);
            return Ok(xml_files);
        }

        // Reference pack names to search (skip OS-specific and NetStandard)
        let ref_packs = vec!["Microsoft.NETCore.App.Ref", "Microsoft.AspNetCore.App.Ref"];

        for pack_name in ref_packs {
            let pack_dir = packs_dir.join(pack_name);

            if !pack_dir.exists() {
                debug!("Pack directory not found: {:?}", pack_dir);
                continue;
            }

            // Find all version directories
            let entries = match std::fs::read_dir(&pack_dir) {
                Ok(entries) => entries,
                Err(e) => {
                    debug!("Failed to read pack directory {:?}: {}", pack_dir, e);
                    continue;
                }
            };

            for entry in entries.filter_map(|e| e.ok()) {
                let version_dir = entry.path();

                // Look in ref/<tfm> subdirectory
                let ref_dir = version_dir.join("ref").join(tfm.as_str());

                if ref_dir.exists() && ref_dir.is_dir() {
                    debug!("Searching for XML files in {:?}", ref_dir);

                    // Find all XML files in this directory
                    match std::fs::read_dir(&ref_dir) {
                        Ok(xml_entries) => {
                            for xml_entry in xml_entries.filter_map(|e| e.ok()) {
                                let path = xml_entry.path();

                                if path.is_file() {
                                    if let Some(extension) = path.extension() {
                                        if extension == "xml" {
                                            debug!("Found XML file: {:?}", path);
                                            xml_files.push(path);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            debug!("Failed to read ref directory {:?}: {}", ref_dir, e);
                        }
                    }
                }
            }
        }

        info!(
            "Found {} C# reference assembly XML files for TFM {} in SDK {:?}",
            xml_files.len(),
            tfm,
            sdk_path
        );
        Ok(xml_files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_old_net_framework() {
        // Test old .NET Framework formats
        assert_eq!(TargetFramework::from_str("v4.5").unwrap().as_str(), "net45");
        assert_eq!(
            TargetFramework::from_str("v4.5.1").unwrap().as_str(),
            "net451"
        );
        assert_eq!(TargetFramework::from_str("v4.6").unwrap().as_str(), "net46");
        assert_eq!(
            TargetFramework::from_str("v4.6.1").unwrap().as_str(),
            "net461"
        );
        assert_eq!(
            TargetFramework::from_str("v4.7.2").unwrap().as_str(),
            "net472"
        );
        assert_eq!(TargetFramework::from_str("v4.8").unwrap().as_str(), "net48");
        assert_eq!(
            TargetFramework::from_str("v4.8.1").unwrap().as_str(),
            "net481"
        );
    }

    #[test]
    fn test_normalize_modern_net() {
        // Test modern .NET (5.0+)
        assert_eq!(
            TargetFramework::from_str("net5.0").unwrap().as_str(),
            "net5.0"
        );
        assert_eq!(
            TargetFramework::from_str("net6.0").unwrap().as_str(),
            "net6.0"
        );
        assert_eq!(
            TargetFramework::from_str("net7.0").unwrap().as_str(),
            "net7.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0").unwrap().as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net9.0").unwrap().as_str(),
            "net9.0"
        );
        assert_eq!(
            TargetFramework::from_str("net10.0").unwrap().as_str(),
            "net10.0"
        );
    }

    #[test]
    fn test_normalize_platform_specific_tfms() {
        // Test platform-specific TFMs - should strip platform suffix
        assert_eq!(
            TargetFramework::from_str("net8.0-android")
                .unwrap()
                .as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0-ios").unwrap().as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0-windows")
                .unwrap()
                .as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0-macos").unwrap().as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0-maccatalyst")
                .unwrap()
                .as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0-tvos").unwrap().as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0-tizen").unwrap().as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0-browser")
                .unwrap()
                .as_str(),
            "net8.0"
        );
    }

    #[test]
    fn test_normalize_platform_specific_with_version() {
        // Test platform-specific TFMs with OS versions
        assert_eq!(
            TargetFramework::from_str("net8.0-ios17.2")
                .unwrap()
                .as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0-android35.0")
                .unwrap()
                .as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0-windows10.0.19041")
                .unwrap()
                .as_str(),
            "net8.0"
        );
    }

    #[test]
    fn test_normalize_netstandard() {
        // Test .NET Standard
        assert_eq!(
            TargetFramework::from_str("netstandard1.0")
                .unwrap()
                .as_str(),
            "netstandard1.0"
        );
        assert_eq!(
            TargetFramework::from_str("netstandard1.1")
                .unwrap()
                .as_str(),
            "netstandard1.1"
        );
        assert_eq!(
            TargetFramework::from_str("netstandard2.0")
                .unwrap()
                .as_str(),
            "netstandard2.0"
        );
        assert_eq!(
            TargetFramework::from_str("netstandard2.1")
                .unwrap()
                .as_str(),
            "netstandard2.1"
        );
    }

    #[test]
    fn test_normalize_netcoreapp() {
        // Test .NET Core
        assert_eq!(
            TargetFramework::from_str("netcoreapp1.0").unwrap().as_str(),
            "netcoreapp1.0"
        );
        assert_eq!(
            TargetFramework::from_str("netcoreapp2.0").unwrap().as_str(),
            "netcoreapp2.0"
        );
        assert_eq!(
            TargetFramework::from_str("netcoreapp2.1").unwrap().as_str(),
            "netcoreapp2.1"
        );
        assert_eq!(
            TargetFramework::from_str("netcoreapp3.0").unwrap().as_str(),
            "netcoreapp3.0"
        );
        assert_eq!(
            TargetFramework::from_str("netcoreapp3.1").unwrap().as_str(),
            "netcoreapp3.1"
        );
    }

    #[test]
    fn test_normalize_net_framework_short_form() {
        // Test .NET Framework short forms
        assert_eq!(
            TargetFramework::from_str("net45").unwrap().as_str(),
            "net45"
        );
        assert_eq!(
            TargetFramework::from_str("net451").unwrap().as_str(),
            "net451"
        );
        assert_eq!(
            TargetFramework::from_str("net46").unwrap().as_str(),
            "net46"
        );
        assert_eq!(
            TargetFramework::from_str("net461").unwrap().as_str(),
            "net461"
        );
        assert_eq!(
            TargetFramework::from_str("net462").unwrap().as_str(),
            "net462"
        );
        assert_eq!(
            TargetFramework::from_str("net47").unwrap().as_str(),
            "net47"
        );
        assert_eq!(
            TargetFramework::from_str("net471").unwrap().as_str(),
            "net471"
        );
        assert_eq!(
            TargetFramework::from_str("net472").unwrap().as_str(),
            "net472"
        );
        assert_eq!(
            TargetFramework::from_str("net48").unwrap().as_str(),
            "net48"
        );
        assert_eq!(
            TargetFramework::from_str("net481").unwrap().as_str(),
            "net481"
        );
    }

    #[test]
    fn test_normalize_invalid_tfms() {
        // Test invalid TFMs
        assert!(TargetFramework::from_str("invalid").is_err());
        assert!(TargetFramework::from_str("net3.0").is_err()); // Too old
        assert!(TargetFramework::from_str("net4.0").is_err()); // Too old
        assert!(TargetFramework::from_str("netstandard3.0").is_err()); // Doesn't exist
        assert!(TargetFramework::from_str("netcoreapp4.0").is_err()); // Doesn't exist
        assert!(TargetFramework::from_str("").is_err());
        assert!(TargetFramework::from_str("net").is_err());
    }

    #[test]
    fn test_normalize_whitespace_handling() {
        // Test whitespace handling
        assert_eq!(
            TargetFramework::from_str(" net8.0 ").unwrap().as_str(),
            "net8.0"
        );
        assert_eq!(
            TargetFramework::from_str("\tnet8.0\n").unwrap().as_str(),
            "net8.0"
        );
    }

    #[test]
    fn test_is_valid_base_tfm() {
        // Valid modern .NET
        assert!(TargetFramework::is_valid_base_tfm("net5.0"));
        assert!(TargetFramework::is_valid_base_tfm("net6.0"));
        assert!(TargetFramework::is_valid_base_tfm("net7.0"));
        assert!(TargetFramework::is_valid_base_tfm("net8.0"));
        assert!(TargetFramework::is_valid_base_tfm("net9.0"));
        assert!(TargetFramework::is_valid_base_tfm("net10.0"));

        // Valid .NET Framework
        assert!(TargetFramework::is_valid_base_tfm("net45"));
        assert!(TargetFramework::is_valid_base_tfm("net451"));
        assert!(TargetFramework::is_valid_base_tfm("net46"));
        assert!(TargetFramework::is_valid_base_tfm("net461"));
        assert!(TargetFramework::is_valid_base_tfm("net462"));
        assert!(TargetFramework::is_valid_base_tfm("net47"));
        assert!(TargetFramework::is_valid_base_tfm("net471"));
        assert!(TargetFramework::is_valid_base_tfm("net472"));
        assert!(TargetFramework::is_valid_base_tfm("net48"));
        assert!(TargetFramework::is_valid_base_tfm("net481"));

        // Valid .NET Standard
        assert!(TargetFramework::is_valid_base_tfm("netstandard1.0"));
        assert!(TargetFramework::is_valid_base_tfm("netstandard1.6"));
        assert!(TargetFramework::is_valid_base_tfm("netstandard2.0"));
        assert!(TargetFramework::is_valid_base_tfm("netstandard2.1"));

        // Valid .NET Core
        assert!(TargetFramework::is_valid_base_tfm("netcoreapp1.0"));
        assert!(TargetFramework::is_valid_base_tfm("netcoreapp2.0"));
        assert!(TargetFramework::is_valid_base_tfm("netcoreapp2.1"));
        assert!(TargetFramework::is_valid_base_tfm("netcoreapp3.0"));
        assert!(TargetFramework::is_valid_base_tfm("netcoreapp3.1"));

        // Invalid TFMs
        assert!(!TargetFramework::is_valid_base_tfm("net3.0")); // Too old
        assert!(!TargetFramework::is_valid_base_tfm("net4.0")); // Too old
        assert!(!TargetFramework::is_valid_base_tfm("netstandard3.0")); // Doesn't exist
        assert!(!TargetFramework::is_valid_base_tfm("netcoreapp4.0")); // Doesn't exist
        assert!(!TargetFramework::is_valid_base_tfm("invalid"));
        assert!(!TargetFramework::is_valid_base_tfm(""));
        assert!(!TargetFramework::is_valid_base_tfm("net"));
        assert!(!TargetFramework::is_valid_base_tfm("net8.0-android")); // Platform suffix not allowed here
    }

    #[test]
    fn test_tfm_to_channel_modern_net() {
        // Modern .NET (5.0+)
        assert_eq!(
            TargetFramework::from_str("net5.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "5.0"
        );
        assert_eq!(
            TargetFramework::from_str("net6.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "6.0"
        );
        assert_eq!(
            TargetFramework::from_str("net7.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "7.0"
        );
        assert_eq!(
            TargetFramework::from_str("net8.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "8.0"
        );
        assert_eq!(
            TargetFramework::from_str("net9.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "9.0"
        );
        assert_eq!(
            TargetFramework::from_str("net10.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "10.0"
        );
    }

    #[test]
    fn test_tfm_to_channel_netcoreapp() {
        // .NET Core
        assert_eq!(
            TargetFramework::from_str("netcoreapp1.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "1.0"
        );
        assert_eq!(
            TargetFramework::from_str("netcoreapp2.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "2.0"
        );
        assert_eq!(
            TargetFramework::from_str("netcoreapp2.1")
                .unwrap()
                .to_channel()
                .unwrap(),
            "2.1"
        );
        assert_eq!(
            TargetFramework::from_str("netcoreapp3.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "3.0"
        );
        assert_eq!(
            TargetFramework::from_str("netcoreapp3.1")
                .unwrap()
                .to_channel()
                .unwrap(),
            "3.1"
        );
    }

    #[test]
    fn test_tfm_to_channel_netstandard() {
        // .NET Standard
        assert_eq!(
            TargetFramework::from_str("netstandard1.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "1.0"
        );
        assert_eq!(
            TargetFramework::from_str("netstandard2.0")
                .unwrap()
                .to_channel()
                .unwrap(),
            "2.0"
        );
        assert_eq!(
            TargetFramework::from_str("netstandard2.1")
                .unwrap()
                .to_channel()
                .unwrap(),
            "2.1"
        );
    }

    #[test]
    fn test_tfm_to_channel_net_framework() {
        // .NET Framework
        assert_eq!(
            TargetFramework::from_str("net45")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.5"
        );
        assert_eq!(
            TargetFramework::from_str("net451")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.5.1"
        );
        assert_eq!(
            TargetFramework::from_str("net46")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.6"
        );
        assert_eq!(
            TargetFramework::from_str("net461")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.6.1"
        );
        assert_eq!(
            TargetFramework::from_str("net462")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.6.2"
        );
        assert_eq!(
            TargetFramework::from_str("net47")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.7"
        );
        assert_eq!(
            TargetFramework::from_str("net471")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.7.1"
        );
        assert_eq!(
            TargetFramework::from_str("net472")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.7.2"
        );
        assert_eq!(
            TargetFramework::from_str("net48")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.8"
        );
        assert_eq!(
            TargetFramework::from_str("net481")
                .unwrap()
                .to_channel()
                .unwrap(),
            "4.8.1"
        );
    }

    #[test]
    fn test_tfm_to_channel_invalid() {
        // Invalid TFMs should error - but we can't create invalid TFMs via from_str
        // So this tests the internal logic
        let invalid_tfm = TargetFramework("invalid".to_string());
        assert!(invalid_tfm.to_channel().is_err());
    }
}
