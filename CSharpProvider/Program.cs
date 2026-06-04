using System.CommandLine;
using System.Net;
using CSharpProvider.Analysis;
using CSharpProvider.Services;
using Microsoft.AspNetCore.Server.Kestrel.Core;

namespace CSharpProvider;

public class Program
{
    public static async Task<int> Main(string[] args)
    {
        var portOption = new Option<int>("--port", () => 14651, "TCP port for gRPC");
        var socketOption = new Option<string?>("--socket", "Unix socket path");
        var nameOption = new Option<string>("--name", () => "c-sharp", "Provider name");
        var contextLinesOption = new Option<int>("--context-lines", () => 10, "Context lines for code snippets");
        var logFileOption = new Option<string?>("--log-file", "Log file path");

        var rootCommand = new RootCommand("C# Analyzer Provider (Roslyn)")
        {
            portOption, socketOption, nameOption, contextLinesOption, logFileOption
        };

        rootCommand.SetHandler(RunServer, portOption, socketOption, nameOption, contextLinesOption, logFileOption);

        return await rootCommand.InvokeAsync(args);
    }

    private static async Task RunServer(int port, string? socket, string name, int contextLines, string? logFile)
    {
        // Using Grpc.AspNetCore so we get: 
        // - Kestrel, an HTTP/2 server
        // - Built-in dependency injection
        // - Middleware pipeline for logging, error handling, etc.

        // ASP.NET Setup Step 1 - DI container setup. "Here are the objects that
        // exist and how to create them."

        var builder = WebApplication.CreateBuilder();

        if (logFile != null)
        {
            builder.Logging.AddFile(logFile);
        }

        builder.WebHost.ConfigureKestrel(options =>
        {
            static void configure(ListenOptions listenOptions) => listenOptions.Protocols = HttpProtocols.Http2;

            if (socket != null)
            {
                options.ListenUnixSocket(socket, configure);
            }
            else
            {
                options.Listen(IPAddress.Any, port, configure);
            }
        });

        builder.Services.AddGrpc();
        // Lets you query the server to discover what services and methods it
        // exposes at runtime, without knowing the proto file.
        builder.Services.AddGrpcReflection();
        // Every gRPC request is handled separately, so we need some singletons
        // to hold shared state and configuration.
        builder.Services.AddSingleton(new ProviderConfig(name, contextLines));
        builder.Services.AddSingleton<PackageResolver>();
        builder.Services.AddSingleton<ProjectStateHolder>();

        // ASP.NET Setup Step 2 - Middleware pipeline setup. "When a request
        // arrives at this path, send it here."

        var app = builder.Build();

        // These services map to the gRPC services defined in provider.proto.
        app.MapGrpcService<ProviderService>();
        app.MapGrpcService<CodeLocationService>();
        // For runtime service discovery.
        app.MapGrpcReflectionService();

        Console.WriteLine($"C# Provider '{name}' listening on {(socket != null ? $"socket {socket}" : $"port {port}")}");

        await app.RunAsync();
    }
}

public record ProviderConfig(string Name, int ContextLines);

// TODO: Switch to Serilog

// Extension method to add our custom file logger to the logging pipeline.
public static class FileLoggerExtensions
{
    public static ILoggingBuilder AddFile(this ILoggingBuilder builder, string path)
    {
        builder.AddProvider(new FileLoggerProvider(path));
        return builder;
    }
}

// A provider that creates file loggers. Used with
// builder.Logging.AddProvider(...).
public class FileLoggerProvider : ILoggerProvider
{
    private readonly string _path;
    private readonly StreamWriter _writer;

    public FileLoggerProvider(string path)
    {
        _path = path;
        _writer = new StreamWriter(path, append: true) { AutoFlush = true };
    }

    public ILogger CreateLogger(string categoryName) => new FileLogger(_writer, categoryName);
    public void Dispose() => _writer.Dispose();
}

// The actual logger that writes log messages to the file. Created by the
// provider for each category.
public class FileLogger : ILogger
{
    private readonly StreamWriter _writer;
    private readonly string _category;

    public FileLogger(StreamWriter writer, string category)
    {
        _writer = writer;
        _category = category;
    }

    public IDisposable? BeginScope<TState>(TState state) where TState : notnull => null;
    public bool IsEnabled(LogLevel logLevel) => logLevel >= LogLevel.Information;

    public void Log<TState>(LogLevel logLevel, EventId eventId, TState state, Exception? exception, Func<TState, Exception?, string> formatter)
    {
        if (!IsEnabled(logLevel))
            return;
        _writer.WriteLine($"[{DateTime.UtcNow:o}] [{logLevel}] [{_category}] {formatter(state, exception)}");
        if (exception != null)
            _writer.WriteLine(exception.ToString());
    }
}
