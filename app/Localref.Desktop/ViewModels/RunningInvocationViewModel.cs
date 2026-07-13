using System;
using CommunityToolkit.Mvvm.Input;
using Localref.Desktop.Services;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>One in-flight plugin invocation shown in the "Running" tab.</summary>
public sealed partial class RunningInvocationViewModel : ViewModelBase
{
    private readonly DaemonService _daemon;
    private readonly Action<string> _reportStatus;

    public RunningInvocationViewModel(
        DaemonService daemon,
        RunningInvocation invocation,
        Action<string> reportStatus)
    {
        _daemon = daemon;
        _reportStatus = reportStatus;
        Invocation = invocation;
        Started = DateTimeOffset
            .FromUnixTimeMilliseconds((long)invocation.startedAtMs)
            .LocalDateTime
            .ToString("HH:mm:ss");
    }

    public RunningInvocation Invocation { get; }
    public ulong Id => Invocation.id;
    public string Plugin => Invocation.plugin;
    public string Action => Invocation.action;
    public string Kind => Invocation.kind;
    public string Started { get; }

    // Cancel this invocation. The daemon fires its cancel signal and the plugin
    // child process is killed; the entry disappears from the list on the next
    // poll (or immediately, since we drop it locally on success).
    [RelayCommand]
    private void Cancel()
    {
        try
        {
            if (_daemon.Handle.CancelPluginRun(Id))
            {
                _reportStatus($"Cancelled {Plugin} · {Action}");
            }
            else
            {
                _reportStatus($"{Plugin} · {Action} already finished");
            }
        }
        catch (Exception ex)
        {
            _reportStatus($"Could not cancel {Plugin}: {ex.Message}");
        }
    }
}
