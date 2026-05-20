using Microsoft.CodeAnalysis.CSharp;

namespace CSharpProvider.Services;

public class ProjectStateHolder
{
    private readonly object _lock = new();
    private ProjectState? _state;

    public ProjectState? Get()
    {
        lock (_lock)
        { return _state; }
    }

    public void Set(ProjectState? state)
    {
        lock (_lock)
        { _state = state; }
    }

    public void Update(Func<ProjectState, ProjectState> updater)
    {
        lock (_lock)
        {
            if (_state != null)
                _state = updater(_state);
        }
    }
}

public record ProjectState(CSharpCompilation Compilation, string ProjectPath);
