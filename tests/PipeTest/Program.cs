using System.IO.Pipes;
using System.Net;
using System.Net.Sockets;
using System.CommandLine;
using Grpc.Net.Client;
using Provider;

var pipeOption = new Option<string>("--pipe", "Named pipe name to connect to") { IsRequired = true };
var repoOption = new Option<string>("--repo", "Path to test repo for Init") { IsRequired = true };
var patternOption = new Option<string>("--pattern", () => "^System(\\..*)$", "Regex pattern for Evaluate");

var rootCommand = new RootCommand("Named pipe gRPC smoke test")
{
    pipeOption, repoOption, patternOption
};

rootCommand.SetHandler(async (string pipeName, string repo, string pattern) =>
{
    var repoPath = Path.GetFullPath(repo);
    Console.WriteLine($"Pipe: {pipeName}");
    Console.WriteLine($"Repo: {repoPath}");
    Console.WriteLine($"Pattern: {pattern}");

    var handler = new SocketsHttpHandler
    {
        ConnectCallback = async (_, ct) =>
        {
            var pipe = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
            await pipe.ConnectAsync(ct);
            return pipe;
        }
    };

    using var channel = GrpcChannel.ForAddress("http://localhost", new GrpcChannelOptions
    {
        HttpHandler = handler
    });

    var client = new ProviderService.ProviderServiceClient(channel);

    // Init
    Console.Write("Init... ");
    var initResponse = await client.InitAsync(new Config
    {
        Location = repoPath,
        AnalysisMode = "source-only"
    }, deadline: DateTime.UtcNow.AddMinutes(10));

    if (!initResponse.Successful)
    {
        Console.WriteLine($"FAIL: {initResponse.Error}");
        Environment.Exit(1);
    }
    Console.WriteLine("PASS");

    // Evaluate
    Console.Write($"Evaluate ({pattern})... ");
    var conditionObj = new { referenced = new { pattern } };
    var conditionInfo = System.Text.Json.JsonSerializer.Serialize(conditionObj);
    var evalResponse = await client.EvaluateAsync(new EvaluateRequest
    {
        Cap = "referenced",
        ConditionInfo = conditionInfo,
        Id = 1
    }, deadline: DateTime.UtcNow.AddMinutes(5));

    if (!evalResponse.Successful)
    {
        Console.WriteLine($"FAIL: {evalResponse.Error}");
        Environment.Exit(1);
    }

    var count = evalResponse.Response?.IncidentContexts.Count ?? 0;
    if (count == 0)
    {
        Console.WriteLine("FAIL: 0 incidents (expected > 0)");
        Environment.Exit(1);
    }
    Console.WriteLine($"PASS ({count} incidents)");

    // Stop
    Console.Write("Stop... ");
    await client.StopAsync(new ServiceRequest { Id = 1 });
    Console.WriteLine("PASS");

    Console.WriteLine("All checks passed");
}, pipeOption, repoOption, patternOption);

return await rootCommand.InvokeAsync(args);
