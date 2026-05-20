using Microsoft.Build.Locator;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.MSBuild;

namespace CSharpProvider.Analysis;

public class ProjectLoader
{
    private readonly ILogger _logger;
    private static bool _msbuildRegistered;
    private static readonly object _registrationLock = new();

    public ProjectLoader(ILogger logger)
    {
        _logger = logger;
    }

    public async Task<CSharpCompilation> LoadAsync(string location, CancellationToken ct = default)
    {
        var msbuildResult = await TryLoadViaMSBuild(location, ct);
        if (msbuildResult != null)
            return msbuildResult;

        _logger.LogInformation("Using ad-hoc compilation for {Location}", location);
        return LoadAdHoc(location);
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

    private async Task<CSharpCompilation> LoadViaMSBuildAsync(string path, bool isSolution, CancellationToken ct)
    {
        EnsureMSBuildRegistered();

        using var workspace = MSBuildWorkspace.Create();
        workspace.WorkspaceFailed += (_, args) =>
            _logger.LogWarning("MSBuild workspace warning: {Message}", args.Diagnostic.Message);

        if (isSolution)
        {
            var solution = await workspace.OpenSolutionAsync(path, cancellationToken: ct);
            var project = solution.Projects.FirstOrDefault(p =>
                p.Language == LanguageNames.CSharp)
                ?? throw new InvalidOperationException("No C# project found in solution");

            var compilation = await project.GetCompilationAsync(ct)
                ?? throw new InvalidOperationException("Failed to get compilation");

            return (CSharpCompilation)compilation;
        }
        else
        {
            var project = await workspace.OpenProjectAsync(path, cancellationToken: ct);
            var compilation = await project.GetCompilationAsync(ct)
                ?? throw new InvalidOperationException("Failed to get compilation");

            return (CSharpCompilation)compilation;
        }
    }

    private CSharpCompilation LoadAdHoc(string location)
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

        var references = DiscoverReferences(location);
        _logger.LogInformation("Discovered {Count} assembly references", references.Count);

        return CSharpCompilation.Create(
            assemblyName: "AdHocAnalysis",
            syntaxTrees: syntaxTrees,
            references: references,
            options: new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary)
                .WithNullableContextOptions(NullableContextOptions.Disable));
    }

    private List<MetadataReference> DiscoverReferences(string location)
    {
        var references = new List<MetadataReference>();

        // Look for DLLs in packages/ directory (NuGet packages)
        var packagesDir = Path.Combine(location, "packages");
        if (Directory.Exists(packagesDir))
        {
            var dlls = Directory.GetFiles(packagesDir, "*.dll", SearchOption.AllDirectories);
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
        }

        // Add basic framework references if available
        var dotnetRefPath = GetFrameworkReferencePath();
        if (dotnetRefPath != null && Directory.Exists(dotnetRefPath))
        {
            var frameworkDlls = Directory.GetFiles(dotnetRefPath, "*.dll");
            foreach (var dll in frameworkDlls)
            {
                try
                {
                    references.Add(MetadataReference.CreateFromFile(dll));
                }
                catch (Exception ex)
                {
                    _logger.LogDebug("Skipping framework assembly: {Dll} ({Error})", dll, ex.Message);
                }
            }
        }

        return references;
    }

    private static string? GetFrameworkReferencePath()
    {
        // Try to find .NET reference assemblies
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
