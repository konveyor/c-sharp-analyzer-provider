using System.CommandLine;
using System.Net;
using CSharpProvider.Services;
using Microsoft.AspNetCore.Server.Kestrel.Core;

var portOption = new Option<int>("--port", () => 14651, "TCP port for gRPC");
var socketOption = new Option<string?>("--socket", "Unix socket path");
var nameOption = new Option<string>("--name", () => "c-sharp", "Provider name");
var contextLinesOption = new Option<int>("--context-lines", () => 10, "Context lines for code snippets");
var logFileOption = new Option<string?>("--log-file", "Log file path");

var rootCommand = new RootCommand("C# Analyzer Provider (Roslyn)")
{
    portOption, socketOption, nameOption, contextLinesOption, logFileOption
};

rootCommand.SetHandler(async (int port, string? socket, string name, int contextLines, string? logFile) =>
{
    var builder = WebApplication.CreateBuilder();

    if (logFile != null)
    {
        builder.Logging.AddFile(logFile);
    }

    builder.WebHost.ConfigureKestrel(options =>
    {
        if (socket != null)
        {
            options.ListenUnixSocket(socket, listenOptions =>
            {
                listenOptions.Protocols = HttpProtocols.Http2;
            });
        }
        else
        {
            options.Listen(IPAddress.Any, port, listenOptions =>
            {
                listenOptions.Protocols = HttpProtocols.Http2;
            });
        }
    });

    builder.Services.AddGrpc();
    builder.Services.AddGrpcReflection();
    builder.Services.AddSingleton(new ProviderConfig(name, contextLines));
    builder.Services.AddSingleton<ProjectStateHolder>();

    var app = builder.Build();

    app.MapGrpcService<ProviderService>();
    app.MapGrpcService<CodeLocationService>();
    app.MapGrpcReflectionService();

    Console.WriteLine($"C# Provider '{name}' listening on {(socket != null ? $"socket {socket}" : $"port {port}")}");
    await app.RunAsync();

}, portOption, socketOption, nameOption, contextLinesOption, logFileOption);

return await rootCommand.InvokeAsync(args);

public record ProviderConfig(string Name, int ContextLines);

// Simple file logging provider
public static class FileLoggerExtensions
{
    public static ILoggingBuilder AddFile(this ILoggingBuilder builder, string path)
    {
        builder.AddProvider(new FileLoggerProvider(path));
        return builder;
    }
}

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
        if (!IsEnabled(logLevel)) return;
        _writer.WriteLine($"[{DateTime.UtcNow:o}] [{logLevel}] [{_category}] {formatter(state, exception)}");
        if (exception != null) _writer.WriteLine(exception.ToString());
    }
}
