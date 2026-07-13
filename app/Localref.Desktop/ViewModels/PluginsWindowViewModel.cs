using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.Linq;
using Avalonia.Threading;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Localref.Desktop.Services;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

public partial class PluginsWindowViewModel : ViewModelBase
{
    private readonly DaemonService? _daemon;
    private readonly DispatcherTimer? _runningTimer;

    public ObservableCollection<PluginManagerItemViewModel> Plugins { get; } = new();
    public ObservableCollection<ScheduledPluginCallViewModel> Schedules { get; } = new();
    public ObservableCollection<RunningInvocationViewModel> Running { get; } = new();
    public ObservableCollection<string> AvailablePluginNames { get; } = new();

    [ObservableProperty]
    private PluginManagerItemViewModel? _selectedPlugin;

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(RemoveScheduleCommand))]
    private ScheduledPluginCallViewModel? _selectedSchedule;

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(AddScheduleCommand))]
    private string _newScheduleId = "";

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(AddScheduleCommand))]
    private string? _newSchedulePlugin;

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(AddScheduleCommand))]
    private string _newScheduleAction = "";

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(AddScheduleCommand))]
    private string _newScheduleExpression = "0 0 3 * * *";

    [ObservableProperty]
    private string _newScheduleParameters = "";

    [ObservableProperty]
    private string _statusText = "Plugin manager ready";

    public PluginsWindowViewModel(DaemonService daemon)
    {
        _daemon = daemon;
        Refresh();
        RefreshRunning();
        // Poll the running list while the window is open so the panel stays live
        // without the daemon pushing per-invocation events. Stopped in Dispose.
        _runningTimer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(1) };
        _runningTimer.Tick += (_, _) => RefreshRunning();
        _runningTimer.Start();
    }

    public PluginsWindowViewModel() { }

    /// Stop the polling timer. Called when the plugins window closes.
    public void Dispose()
    {
        _runningTimer?.Stop();
    }

    // Refresh the list of currently-running plugin invocations. Runs on the UI
    // thread (driven by the dispatcher timer); the FFI read is a cheap in-memory
    // registry snapshot, so no Task.Run is needed. Preserves nothing across
    // refreshes — the set is small and fully replaced each tick.
    public void RefreshRunning()
    {
        if (_daemon is null || !_daemon.IsRunning) return;
        try
        {
            var invocations = _daemon.Handle.ListRunningPlugins();
            Running.Clear();
            foreach (var invocation in invocations)
            {
                Running.Add(new RunningInvocationViewModel(
                    _daemon,
                    invocation,
                    message => StatusText = message));
            }
        }
        catch (Exception)
        {
            // Transient read failures (e.g. during shutdown) are ignored; the
            // next tick recovers.
        }
    }

    // Initial in-memory load when the window opens. Rediscovery from disk is
    // the "Scan for plugins" command (Rescan); there is no separate refresh
    // button because a scan is a strict superset of a refresh.
    private void Refresh()
    {
        if (_daemon is null) return;
        try
        {
            Populate(_daemon.Handle.ListPlugins());
            StatusText = Plugins.Count == 1 ? "1 plugin installed" : $"{Plugins.Count} plugins installed";
        }
        catch (Exception ex)
        {
            StatusText = $"Could not load plugins: {ex.Message}";
        }
    }

    // Rediscover plugins from disk so a freshly deployed plugin (e.g. one
    // dropped into the plugins dir after the daemon started) loads without a
    // restart. Runs off the UI thread because RescanPlugins rebuilds the search
    // index synchronously on the daemon side.
    [RelayCommand]
    public async System.Threading.Tasks.Task Rescan()
    {
        if (_daemon is null) return;
        StatusText = "Scanning for plugins...";
        try
        {
            var descriptors = await System.Threading.Tasks.Task.Run(
                () => _daemon.Handle.RescanPlugins());
            Populate(descriptors);
            StatusText = Plugins.Count == 1 ? "1 plugin installed" : $"{Plugins.Count} plugins installed";
        }
        catch (Exception ex)
        {
            StatusText = $"Could not scan for plugins: {ex.Message}";
        }
    }

    // Repopulate the plugin list, available names, and schedules from a fresh
    // descriptor set, preserving the current selection where possible.
    private void Populate(PluginDescriptor[] descriptors)
    {
        if (_daemon is null) return;
        var selectedName = SelectedPlugin?.Name;
        Plugins.Clear();
        AvailablePluginNames.Clear();
        foreach (var descriptor in descriptors)
        {
            Plugins.Add(new PluginManagerItemViewModel(
                _daemon,
                descriptor,
                message => StatusText = message));
            AvailablePluginNames.Add(descriptor.name);
        }
        SelectedPlugin = Plugins.FirstOrDefault(item => item.Name == selectedName) ?? Plugins.FirstOrDefault();
        if (NewSchedulePlugin is null || !AvailablePluginNames.Contains(NewSchedulePlugin))
        {
            NewSchedulePlugin = AvailablePluginNames.FirstOrDefault();
        }
        LoadSchedules(descriptors);
    }

    private void LoadSchedules(PluginDescriptor[]? descriptors = null)
    {
        if (_daemon is null) return;
        descriptors ??= _daemon.Handle.ListPlugins();
        var selectedId = SelectedSchedule?.Id;
        var selectedPlugin = SelectedSchedule?.Plugin;
        Schedules.Clear();
        foreach (var descriptor in descriptors)
        {
            foreach (var jobId in descriptor.cron)
            {
                Schedules.Add(new ScheduledPluginCallViewModel(descriptor.name, jobId));
            }
        }
        foreach (var call in _daemon.Handle.ListSchedules())
        {
            Schedules.Add(new ScheduledPluginCallViewModel(call));
        }
        SelectedSchedule = Schedules.FirstOrDefault(schedule =>
            schedule.Id == selectedId && schedule.Plugin == selectedPlugin);
    }

    private bool CanAddSchedule() =>
        _daemon is not null &&
        !string.IsNullOrWhiteSpace(NewScheduleId) &&
        !string.IsNullOrWhiteSpace(NewSchedulePlugin) &&
        !string.IsNullOrWhiteSpace(NewScheduleAction) &&
        !string.IsNullOrWhiteSpace(NewScheduleExpression);

    [RelayCommand(CanExecute = nameof(CanAddSchedule))]
    private void AddSchedule()
    {
        if (_daemon is null || NewSchedulePlugin is null) return;
        try
        {
            _daemon.Handle.RegisterSchedule(new ScheduledCall(
                id: NewScheduleId.Trim(),
                plugin: NewSchedulePlugin,
                action: NewScheduleAction.Trim(),
                @params: ParseParameters(NewScheduleParameters),
                schedule: NewScheduleExpression.Trim()));
            StatusText = $"Schedule {NewScheduleId.Trim()} registered";
            NewScheduleId = "";
            NewScheduleAction = "";
            NewScheduleParameters = "";
            LoadSchedules();
        }
        catch (Exception ex)
        {
            StatusText = $"Could not register schedule: {ex.Message}";
        }
    }

    private bool CanRemoveSchedule() => SelectedSchedule?.IsRuntime == true && _daemon is not null;

    [RelayCommand(CanExecute = nameof(CanRemoveSchedule))]
    private void RemoveSchedule()
    {
        if (_daemon is null || SelectedSchedule is not { IsRuntime: true } schedule) return;
        try
        {
            _daemon.Handle.RemoveSchedule(schedule.Id);
            StatusText = $"Schedule {schedule.Id} removed";
            LoadSchedules();
        }
        catch (Exception ex)
        {
            StatusText = $"Could not remove schedule: {ex.Message}";
        }
    }

    private static Dictionary<string, string> ParseParameters(string text)
    {
        var result = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var entry in text.Split(['\r', '\n', ';'], StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            var separator = entry.IndexOf('=');
            if (separator > 0)
            {
                result[entry[..separator].Trim()] = entry[(separator + 1)..].Trim();
            }
        }
        return result;
    }

}
