using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.Input;
using Localref.Desktop.Services;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>A top-level plugin action executed against the captured library selection.</summary>
public partial class PluginActionViewModel : ViewModelBase
{
    private readonly DaemonService _daemon;
    private readonly string _plugin;
    private readonly UiAction _action;
    private readonly IReadOnlyList<string> _selectedIds;
    private readonly string? _activeId;
    private readonly Action<string> _reportStatus;
    private readonly Func<string, string, Task> _save;

    public PluginActionViewModel(
        DaemonService daemon,
        string plugin,
        UiAction action,
        IReadOnlyList<string> selectedIds,
        string? activeId,
        Action<string> reportStatus,
        Func<string, string, Task> save)
    {
        _daemon = daemon;
        _plugin = plugin;
        _action = action;
        _selectedIds = selectedIds;
        _activeId = activeId;
        _reportStatus = reportStatus;
        _save = save;
    }

    public string Label => _action.label;

    [RelayCommand]
    public async Task Run()
    {
        var form = new Dictionary<string, string>
        {
            ["selected"] = string.Join(',', _selectedIds),
        };
        if (!string.IsNullOrWhiteSpace(_activeId))
        {
            form["active"] = _activeId;
        }

        try
        {
            var result = await Task.Run(() =>
                _daemon.Handle.RunPluginAction(_plugin, _action.id, form));
            if (result.status != "ok")
            {
                _reportStatus(result.message ?? $"{Label} failed");
                return;
            }
            if (result.result is { } content && result.filename is { } filename)
            {
                await _save(filename, content);
            }
            _reportStatus(result.message ?? $"{Label} completed");
        }
        catch (Exception ex)
        {
            ExceptionService.Current.Report(ex, $"Run plugin action {_plugin}/{_action.id}", ExceptionSource.FFI);
            _reportStatus($"Could not run {Label}: {ex.Message}");
        }
    }
}
