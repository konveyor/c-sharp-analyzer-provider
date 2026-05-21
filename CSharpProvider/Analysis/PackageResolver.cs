using System.Xml.Linq;
using Microsoft.CodeAnalysis;
using NuGet.Common;
using NuGet.Frameworks;
using NuGet.Packaging;
using NuGet.Protocol;
using NuGet.Protocol.Core.Types;
using NuGet.Versioning;

namespace CSharpProvider.Analysis;

public class PackageResolver
{
    private readonly ILogger<PackageResolver> _logger;
    private readonly string _cacheDir;
    private readonly SourceRepository _repository;

    public PackageResolver(ILogger<PackageResolver> logger, string? cacheDir = null)
    {
        _logger = logger;
        _cacheDir = cacheDir ?? Path.Combine(Path.GetTempPath(), "csharp-provider-cache");
        Directory.CreateDirectory(_cacheDir);
        _logger.LogInformation("Package cache directory: {CacheDir}", _cacheDir);

        _repository = Repository.Factory.GetCoreV3("https://api.nuget.org/v3/index.json");
    }

    public async Task<List<MetadataReference>> ResolveAsync(
        List<PackageIdentity> packages, string tfm, CancellationToken ct = default)
    {
        var references = new List<MetadataReference>();
        var framework = NuGetFramework.Parse(tfm);
        var resource = await _repository.GetResourceAsync<FindPackageByIdResource>(ct);

        foreach (var package in packages)
        {
            try
            {
                var dlls = await DownloadAndExtractAsync(resource, package, framework, ct);
                foreach (var dll in dlls)
                {
                    try
                    {
                        references.Add(MetadataReference.CreateFromFile(dll));
                    }
                    catch (Exception ex)
                    {
                        _logger.LogDebug("Skipping invalid assembly {Dll}: {Error}", dll, ex.Message);
                    }
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning("Failed to resolve {Package} {Version}: {Error}",
                    package.Id, package.Version, ex.Message);
            }
        }

        return references;
    }

    private async Task<List<string>> DownloadAndExtractAsync(
        FindPackageByIdResource resource, PackageIdentity package,
        NuGetFramework framework, CancellationToken ct)
    {
        var packageDir = Path.Combine(_cacheDir, $"{package.Id}.{package.Version}");
        var libDir = Path.Combine(packageDir, "lib");

        if (Directory.Exists(libDir))
        {
            _logger.LogDebug("Using cached {Package} {Version}", package.Id, package.Version);
            return FindDlls(packageDir, framework);
        }

        _logger.LogInformation("Downloading {Package} {Version}", package.Id, package.Version);

        var nupkgPath = Path.Combine(_cacheDir, $"{package.Id}.{package.Version}.nupkg");

        if (!File.Exists(nupkgPath))
        {
            using var stream = File.Create(nupkgPath);
            var found = await resource.CopyNupkgToStreamAsync(
                package.Id, package.Version, stream,
                new SourceCacheContext(), NullLogger.Instance, ct);

            if (!found)
                throw new InvalidOperationException($"Package not found on nuget.org");
        }

        Directory.CreateDirectory(packageDir);

        using var packageReader = new PackageArchiveReader(nupkgPath);
        foreach (var file in await packageReader.GetFilesAsync(ct))
        {
            var targetPath = Path.Combine(packageDir, file);
            var targetDir = Path.GetDirectoryName(targetPath)!;
            Directory.CreateDirectory(targetDir);

            using var fileStream = packageReader.GetStream(file);
            using var target = File.Create(targetPath);
            await fileStream.CopyToAsync(target, ct);
        }

        return FindDlls(packageDir, framework);
    }

    private List<string> FindDlls(string packageDir, NuGetFramework framework)
    {
        var libDir = Path.Combine(packageDir, "lib");
        if (!Directory.Exists(libDir))
        {
            var buildDir = Path.Combine(packageDir, "build");
            if (Directory.Exists(buildDir))
                return FindDllsInFrameworkDirs(buildDir, framework);

            var refDir = Path.Combine(packageDir, "ref");
            if (Directory.Exists(refDir))
                return FindDllsInFrameworkDirs(refDir, framework);

            return new List<string>();
        }

        return FindDllsInFrameworkDirs(libDir, framework);
    }

    private List<string> FindDllsInFrameworkDirs(string baseDir, NuGetFramework targetFramework)
    {
        var frameworkDirs = Directory.GetDirectories(baseDir)
            .Select(d => new
            {
                Path = d,
                Framework = NuGetFramework.Parse(Path.GetFileName(d))
            })
            .Where(d => d.Framework != NuGetFramework.UnsupportedFramework)
            .OrderByDescending(d => d.Framework.Version)
            .ToList();

        var compatible = frameworkDirs
            .FirstOrDefault(d => DefaultCompatibilityProvider.Instance
                .IsCompatible(targetFramework, d.Framework));

        if (compatible == null)
        {
            if (frameworkDirs.Count > 0)
            {
                compatible = frameworkDirs.First();
                _logger.LogDebug("No compatible TFM found in {Dir}, using {Fallback}",
                    baseDir, Path.GetFileName(compatible.Path));
            }
            else
            {
                return new List<string>();
            }
        }

        return Directory.GetFiles(compatible.Path, "*.dll", SearchOption.AllDirectories).ToList();
    }

    public static List<PackageIdentity> ParsePackagesConfig(string path)
    {
        var doc = XDocument.Load(path);
        return doc.Root!.Elements("package")
            .Select(e => new PackageIdentity(
                e.Attribute("id")!.Value,
                NuGetVersion.Parse(e.Attribute("version")!.Value)))
            .ToList();
    }

    public static List<PackageIdentity> ParsePaketLock(string path)
    {
        var packages = new List<PackageIdentity>();
        var inNuget = false;

        foreach (var line in File.ReadLines(path))
        {
            if (line.TrimStart().Length == 0)
                continue;

            if (line == "NUGET")
            {
                inNuget = true;
                continue;
            }

            if (!line.StartsWith(" ") && line != "NUGET")
            {
                inNuget = false;
                continue;
            }

            if (!inNuget)
                continue;

            if (line.StartsWith("    remote:"))
                continue;

            var trimmed = line.TrimStart();

            // Top-level packages have 4 spaces of indent, dependencies have 6
            var indent = line.Length - trimmed.Length;
            if (indent != 4)
                continue;

            var parenIdx = trimmed.IndexOf('(');
            if (parenIdx < 0)
                continue;

            var name = trimmed[..parenIdx].Trim();
            var versionStr = trimmed[(parenIdx + 1)..trimmed.IndexOf(')')].Trim();

            if (NuGetVersion.TryParse(versionStr, out var version))
            {
                packages.Add(new PackageIdentity(name, version));
            }
        }

        return packages;
    }

    public static List<PackageIdentity> ParsePackageReferences(string csprojPath)
    {
        var doc = XDocument.Load(csprojPath);
        var ns = doc.Root!.Name.Namespace;

        return doc.Descendants(ns + "PackageReference")
            .Where(e => e.Attribute("Include") != null && e.Attribute("Version") != null)
            .Select(e => new PackageIdentity(
                e.Attribute("Include")!.Value,
                NuGetVersion.Parse(e.Attribute("Version")!.Value)))
            .ToList();
    }
}

public record PackageIdentity(string Id, NuGetVersion Version);
