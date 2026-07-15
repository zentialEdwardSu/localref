using System;
using CommunityToolkit.Mvvm.Input;
using Localref.Desktop.Services;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>One manageable plugin plus its declarative native pages.</summary>
public sealed partial class PluginManagerItemViewModel : ViewModelBase
{
    private readonly DaemonService _daemon;
    private readonly Action<string> _reportStatus;
    private bool _isEnabled;

    public PluginManagerItemViewModel(
        DaemonService daemon,
        PluginDescriptor descriptor,
        Action<string> reportStatus)
    {
        _daemon = daemon;
        _reportStatus = reportStatus;
        Descriptor = descriptor;
        _isEnabled = descriptor.enabled;
    }

    public PluginDescriptor Descriptor { get; }
    public string Name => Descriptor.name;
    public string Description => Descriptor.description ?? "No description provided";
    public string Version => Descriptor.version is { } v ? $"v{v}" : "—";
    public string Directory => Descriptor.dir;
    public string Hooks => Descriptor.hooks.Length == 0 ? "None" : string.Join(", ", Descriptor.hooks);
    public string Schedules => Descriptor.cron.Length == 0 ? "None" : string.Join(", ", Descriptor.cron);
    public int SurfaceCount => (Descriptor.ui?.actions.Length ?? 0) + (Descriptor.ui?.pages.Length ?? 0);
    public string SurfaceSummary => SurfaceCount == 1 ? "1 surface" : $"{SurfaceCount} surfaces";
    public string SurfaceDescription => Descriptor.ui is not { } ui
        ? "No native UI declared"
        : $"{ui.actions.Length} action(s), {ui.pages.Length} page(s)";

    public bool IsEnabled
    {
        get => _isEnabled;
        set
        {
            if (_isEnabled == value)
            {
                return;
            }

            var previous = _isEnabled;
            SetProperty(ref _isEnabled, value);
            try
            {
                _daemon.Handle.SetPluginEnabled(Name, value);
                _reportStatus($"{Name} {(value ? "enabled" : "disabled")}");
            }
            catch (Exception ex)
            {
                ExceptionService.Current.Report(ex, $"Set plugin enabled: {Name}", ExceptionSource.FFI);
                SetProperty(ref _isEnabled, previous);
                _reportStatus($"Could not update {Name}: {ex.Message}");
            }
        }
    }

    // Open this plugin's directory in the platform file manager. The daemon
    // resolves the path from the discovered manifest, not from this VM.
    [RelayCommand]
    private void OpenFolder()
    {
        try
        {
            if (!_daemon.Handle.OpenPluginFolder(Name))
            {
                _reportStatus($"{Name} folder not found");
            }
        }
        catch (Exception ex)
        {
            ExceptionService.Current.Report(ex, $"Open plugin folder: {Name}", ExceptionSource.FFI);
            _reportStatus($"Could not open {Name} folder: {ex.Message}");
        }
    }
}
