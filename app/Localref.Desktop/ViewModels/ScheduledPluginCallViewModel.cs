using System.Linq;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

public sealed class ScheduledPluginCallViewModel
{
    public ScheduledPluginCallViewModel(ScheduledCall call)
    {
        Call = call;
        Id = call.id;
        Plugin = call.plugin;
        Action = call.action;
        Schedule = call.schedule;
        Parameters = call.@params.Count == 0
            ? "—"
            : string.Join(", ", call.@params.Select(pair => $"{pair.Key}={pair.Value}"));
        IsRuntime = true;
    }

    public ScheduledPluginCallViewModel(string plugin, string jobId)
    {
        Id = jobId;
        Plugin = plugin;
        Action = $"cron {jobId}";
        Schedule = "Defined in plugin.toml";
        Parameters = "—";
        IsRuntime = false;
    }

    public ScheduledCall? Call { get; }
    public string Id { get; }
    public string Plugin { get; }
    public string Action { get; }
    public string Schedule { get; }
    public string Parameters { get; }
    public bool IsRuntime { get; }
    public string Source => IsRuntime ? "Runtime" : "Manifest";
}
