using System.Xml.Linq;
using Microsoft.Build.Locator;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.MSBuild;
using NuGet.Versioning;

namespace CSharpProvider.Analysis;

public class ProjectLoader
{
    private readonly ILogger _logger;
    private readonly PackageResolver _packageResolver;
    private static bool _msbuildRegistered;
    private static readonly object _registrationLock = new();

    public ProjectLoader(ILogger logger, PackageResolver packageResolver)
    {
        _logger = logger;
        _packageResolver = packageResolver;
    }

    public async Task<CSharpCompilation> LoadAsync(string location, CancellationToken ct = default)
    {
        var msbuildResult = await TryLoadViaMSBuild(location, ct);
        if (msbuildResult != null)
            return msbuildResult;

        _logger.LogInformation("Using ad-hoc compilation for {Location}", location);
        return await LoadAdHocAsync(location, ct);
    }

    private async Task<CSharpCompilation?> TryLoadViaMSBuild(string location, CancellationToken ct)
    {
        var slnFiles = Directory.GetFiles(location, "*.sln", SearchOption.TopDirectoryOnly);
        if (slnFiles.Length > 0)
        {
            try
            {
                _logger.LogInformation("Found .sln file, attempting MSBuildWorkspace: {Sln}", slnFiles[0]);
                return await LoadViaMSBuildAsync(slnFiles[0], isSolution: true, ct);
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "MSBuildWorkspace failed for .sln, falling back");
            }
        }

        var csprojFiles = Directory.GetFiles(location, "*.csproj", SearchOption.AllDirectories);
        foreach (var csproj in csprojFiles)
        {
            if (!IsSdkStyle(csproj))
                continue;

            try
            {
                _logger.LogInformation("Found SDK-style .csproj, attempting MSBuildWorkspace: {Csproj}", csproj);
                return await LoadViaMSBuildAsync(csproj, isSolution: false, ct);
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "MSBuildWorkspace failed for .csproj, falling back");
            }
        }

        return null;
    }

    // TODO: use the NuGet SDK (NuGet.Commands.RestoreRunner) instead of shelling out.
    // The programmatic MSBuild restore via BuildManager hits SDK version mismatches
    // because MSBuildLocator picks up a different MSBuild than the dotnet CLI uses.
    private void RunDotnetRestore(string path)
    {
        _logger.LogInformation("Running dotnet restore on {Path}", path);
        var process = new System.Diagnostics.Process
        {
            StartInfo = new System.Diagnostics.ProcessStartInfo
            {
                FileName = "dotnet",
                ArgumentList = { "restore", path },
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
            }
        };
        process.Start();
        process.StandardOutput.ReadToEnd();
        var stderr = process.StandardError.ReadToEnd();
        process.WaitForExit();
        if (process.ExitCode != 0)
        {
            _logger.LogWarning("dotnet restore failed (exit {Code}): {Stderr}",
                process.ExitCode, stderr.Length > 500 ? stderr[..500] : stderr);
            throw new InvalidOperationException($"dotnet restore failed with exit code {process.ExitCode}");
        }
        _logger.LogInformation("dotnet restore succeeded for {Path}", path);
    }

    private async Task<CSharpCompilation> LoadViaMSBuildAsync(string path, bool isSolution, CancellationToken ct)
    {
        try
        {
            RunDotnetRestore(path);
        }
        catch (Exception ex)
        {
            _logger.LogWarning("dotnet restore failed, continuing anyway: {Message}", ex.Message);
        }
        EnsureMSBuildRegistered();

        using var workspace = MSBuildWorkspace.Create();
        workspace.WorkspaceFailed += (_, args) =>
            _logger.LogWarning("MSBuild workspace warning: {Message}", args.Diagnostic.Message);

        CSharpCompilation compilation;
        if (isSolution)
        {
            var solution = await workspace.OpenSolutionAsync(path, cancellationToken: ct);
            var project = solution.Projects.FirstOrDefault(p =>
                p.Language == LanguageNames.CSharp)
                ?? throw new InvalidOperationException("No C# project found in solution");

            compilation = (CSharpCompilation)(await project.GetCompilationAsync(ct)
                ?? throw new InvalidOperationException("Failed to get compilation"));
        }
        else
        {
            var project = await workspace.OpenProjectAsync(path, cancellationToken: ct);
            compilation = (CSharpCompilation)(await project.GetCompilationAsync(ct)
                ?? throw new InvalidOperationException("Failed to get compilation"));
        }

        if (!compilation.References.Any())
        {
            throw new InvalidOperationException(
                "MSBuild produced a compilation with no references — framework assemblies are missing");
        }

        var diagnostics = compilation.GetDiagnostics();
        var errors = diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error).ToList();
        _logger.LogInformation("MSBuild compilation has {Count} references, {ErrorCount} compilation-level errors",
            compilation.References.Count(), errors.Count);
        var treeErrors = compilation.SyntaxTrees.Sum(t =>
            compilation.GetSemanticModel(t).GetDiagnostics().Count(d =>
                d.Severity == DiagnosticSeverity.Error));
        _logger.LogInformation("Total semantic errors across all trees: {Count}", treeErrors);
        if (errors.Count > 0)
        {
            var sample = errors.Take(5).Select(d => $"{d.Id}: {d.GetMessage()}");
            _logger.LogWarning("Sample errors: {Errors}", string.Join("; ", sample));
        }
        var unresolvedTypes = errors.Count(d => d.Id == "CS0246" || d.Id == "CS0234");
        if (unresolvedTypes > 0)
        {
            _logger.LogWarning(
                "MSBuild compilation has {Count} unresolved type/namespace errors, falling back to ad-hoc",
                unresolvedTypes);
            throw new InvalidOperationException(
                $"MSBuild compilation has {unresolvedTypes} unresolved type errors — NuGet packages likely missing");
        }

        return compilation;
    }

    private async Task<CSharpCompilation> LoadAdHocAsync(string location, CancellationToken ct)
    {
        var csFiles = Directory.GetFiles(location, "*.cs", SearchOption.AllDirectories);
        _logger.LogInformation("Found {Count} .cs files for ad-hoc compilation", csFiles.Length);

        var syntaxTrees = new List<SyntaxTree>();
        foreach (var file in csFiles)
        {
            var source = File.ReadAllText(file);
            var tree = CSharpSyntaxTree.ParseText(source, path: file);
            syntaxTrees.Add(tree);
        }

        var references = await DiscoverReferencesAsync(location, ct);
        _logger.LogInformation("Discovered {Count} assembly references", references.Count);

        return CSharpCompilation.Create(
            assemblyName: "AdHocAnalysis",
            syntaxTrees: syntaxTrees,
            references: references,
            options: new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary)
                .WithNullableContextOptions(NullableContextOptions.Disable));
    }

    private async Task<List<MetadataReference>> DiscoverReferencesAsync(
        string location, CancellationToken ct)
    {
        var tfm = DetectTargetFramework(location);
        _logger.LogInformation("Detected target framework: {Tfm}", tfm ?? "unknown");

        var packages = new List<PackageIdentity>();

        // Add framework reference assemblies for .NET Framework projects
        if (IsNetFramework(tfm))
        {
            packages.Add(new PackageIdentity(
                $"Microsoft.NETFramework.ReferenceAssemblies.{tfm}",
                NuGetVersion.Parse("1.0.3")));
        }

        // Discover third-party packages from whichever format exists
        packages.AddRange(DiscoverPackages(location));

        var references = new List<MetadataReference>();

        // Download and resolve NuGet packages
        if (packages.Count > 0 && tfm != null)
        {
            var packageRefs = await _packageResolver.ResolveAsync(packages, tfm, ct);
            references.AddRange(packageRefs);
        }

        // For modern .NET, use on-disk SDK reference assemblies
        if (!IsNetFramework(tfm))
        {
            var frameworkPath = GetFrameworkReferencePath();
            if (frameworkPath != null)
            {
                references.AddRange(LoadDllsFromDirectory(frameworkPath));
            }
        }

        // Scan for DLLs already on disk
        references.AddRange(ScanOnDiskDlls(location));

        return DeduplicateReferences(references);
    }

    private List<PackageIdentity> DiscoverPackages(string location)
    {
        // Check in priority order: paket.lock, packages.config, PackageReference

        // Walk up to find paket.lock (it's usually at the solution root)
        var paketLock = FindFileUpward(location, "paket.lock");
        if (paketLock != null)
        {
            _logger.LogInformation("Found paket.lock at {Path}", paketLock);
            return PackageResolver.ParsePaketLock(paketLock);
        }

        var packagesConfig = FindFile(location, "packages.config");
        if (packagesConfig != null)
        {
            _logger.LogInformation("Found packages.config at {Path}", packagesConfig);
            return PackageResolver.ParsePackagesConfig(packagesConfig);
        }

        var csprojFiles = Directory.GetFiles(location, "*.csproj", SearchOption.AllDirectories);
        foreach (var csproj in csprojFiles)
        {
            var refs = PackageResolver.ParsePackageReferences(csproj);
            if (refs.Count > 0)
            {
                _logger.LogInformation("Found {Count} PackageReferences in {Path}", refs.Count, csproj);
                return refs;
            }
        }

        return new List<PackageIdentity>();
    }

    private List<MetadataReference> ScanOnDiskDlls(string location)
    {
        var references = new List<MetadataReference>();

        string[] dirsToScan = ["packages", "lib"];
        foreach (var dirName in dirsToScan)
        {
            var dir = Path.Combine(location, dirName);
            if (Directory.Exists(dir))
            {
                _logger.LogInformation("Scanning {Dir} for DLLs", dir);
                references.AddRange(LoadDllsFromDirectory(dir, SearchOption.AllDirectories));
            }
        }

        return references;
    }

    private List<MetadataReference> LoadDllsFromDirectory(
        string dir, SearchOption searchOption = SearchOption.TopDirectoryOnly)
    {
        var references = new List<MetadataReference>();
        var dlls = Directory.GetFiles(dir, "*.dll", searchOption);
        foreach (var dll in dlls)
        {
            try
            {
                references.Add(MetadataReference.CreateFromFile(dll));
            }
            catch (Exception ex)
            {
                _logger.LogDebug("Skipping invalid assembly: {Dll} ({Error})", dll, ex.Message);
            }
        }
        return references;
    }

    private static List<MetadataReference> DeduplicateReferences(List<MetadataReference> references)
    {
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var result = new List<MetadataReference>();

        foreach (var r in references)
        {
            if (r is PortableExecutableReference peRef && peRef.FilePath != null)
            {
                var fileName = Path.GetFileName(peRef.FilePath);
                if (seen.Add(fileName))
                    result.Add(r);
            }
            else
            {
                result.Add(r);
            }
        }

        return result;
    }

    internal static string? DetectTargetFramework(string location)
    {
        var csprojFiles = Directory.GetFiles(location, "*.csproj", SearchOption.AllDirectories);
        foreach (var csproj in csprojFiles)
        {
            try
            {
                var doc = XDocument.Load(csproj);
                var ns = doc.Root!.Name.Namespace;

                // SDK-style: <TargetFramework>net8.0</TargetFramework>
                var tfElement = doc.Descendants(ns + "TargetFramework").FirstOrDefault();
                if (tfElement != null)
                    return tfElement.Value.Trim();

                // Legacy: <TargetFrameworkVersion>v4.5</TargetFrameworkVersion>
                var tfvElement = doc.Descendants(ns + "TargetFrameworkVersion").FirstOrDefault();
                if (tfvElement != null)
                    return NormalizeFrameworkVersion(tfvElement.Value.Trim());
            }
            catch (Exception)
            {
                // Skip malformed .csproj files
            }
        }

        return null;
    }

    internal static bool IsNetFramework(string? tfm)
    {
        // .NET Framework TFMs: net20, net35, net40, net45, net451, net472, etc.
        // Modern .NET TFMs: net5.0, net6.0, net8.0, net9.0 (contain a dot)
        return tfm != null
            && tfm.StartsWith("net")
            && !tfm.Contains('.');
    }

    internal static string NormalizeFrameworkVersion(string version)
    {
        // v3.5 -> net35, v4.5 -> net45, v4.5.1 -> net451, v4.7.2 -> net472
        var v = version.TrimStart('v', 'V');
        var cleaned = v.Replace(".", "");
        return $"net{cleaned}";
    }

    private static string? FindFile(string location, string fileName)
    {
        var files = Directory.GetFiles(location, fileName, SearchOption.AllDirectories);
        return files.Length > 0 ? files[0] : null;
    }

    private static string? FindFileUpward(string location, string fileName)
    {
        var dir = location;
        while (dir != null)
        {
            var candidate = Path.Combine(dir, fileName);
            if (File.Exists(candidate))
                return candidate;
            dir = Directory.GetParent(dir)?.FullName;
        }
        return null;
    }

    private static string? GetFrameworkReferencePath()
    {
        var dotnetRoot = Environment.GetEnvironmentVariable("DOTNET_ROOT")
            ?? "/usr/share/dotnet";

        var refPath = Path.Combine(dotnetRoot, "packs", "Microsoft.NETCore.App.Ref");
        if (Directory.Exists(refPath))
        {
            var versions = Directory.GetDirectories(refPath)
                .OrderByDescending(d => d)
                .FirstOrDefault();
            if (versions != null)
            {
                var refDir = Directory.GetDirectories(Path.Combine(versions, "ref"))
                    .FirstOrDefault();
                if (refDir != null)
                    return refDir;
            }
        }

        return null;
    }

    private static bool IsSdkStyle(string csprojPath)
    {
        try
        {
            var content = File.ReadAllText(csprojPath);
            return content.Contains("<Project Sdk=", StringComparison.OrdinalIgnoreCase);
        }
        catch
        {
            return false;
        }
    }

    private static void EnsureMSBuildRegistered()
    {
        lock (_registrationLock)
        {
            if (!_msbuildRegistered)
            {
                MSBuildLocator.RegisterDefaults();
                _msbuildRegistered = true;
            }
        }
    }
}
