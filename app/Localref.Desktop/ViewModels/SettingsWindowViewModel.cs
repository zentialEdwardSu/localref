using System;
using System.Net.Http;
using System.Net.Http.Json;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Localref.Desktop.Services;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

public partial class SettingsWindowViewModel : ViewModelBase
{
    private readonly DaemonService? _daemon;
    private readonly WindowsStartupService _startupService = new();
    private readonly WindowsStartMenuService _startMenuService = new();
    private bool _quietStart = true;
    private static readonly HttpClient HttpClient = new();

    [ObservableProperty] private string _configPath = "";
    [ObservableProperty] private string _repoName = "Localref";
    [ObservableProperty] private string _libraryRoot = "";
    [ObservableProperty] private string _restAddr = "127.0.0.1:24817";
    [ObservableProperty] private string _restEndpoint = "http://127.0.0.1:24817";
    [ObservableProperty] private string _cscAddr = "127.0.0.1:23119";
    [ObservableProperty] private bool _launchAtStartup;
    [ObservableProperty] private bool _startHidden;
    [ObservableProperty] private string _statusText = "Settings are stored in config.toml";

    public SettingsWindowViewModel(DaemonService daemon)
    {
        _daemon = daemon;
        Load();
    }

    public SettingsWindowViewModel() { }

    [RelayCommand]
    private void Load()
    {
        try
        {
            var settings = LocalrefFfiMethods.LoadAppSettings();
            ConfigPath = settings.configPath;
            RepoName = settings.repoName;
            LibraryRoot = settings.libraryRoot;
            RestAddr = settings.restAddr;
            RestEndpoint = settings.restEndpoint;
            CscAddr = settings.cscAddr;
            StartHidden = settings.startHidden;
            _quietStart = settings.quietStart;
            LaunchAtStartup = _startupService.IsEnabled();
            StatusText = "Settings loaded";
        }
        catch (Exception ex)
        {
            ExceptionService.Current.Report(ex, "Load settings", ExceptionSource.FFI);
            StatusText = $"Could not load settings: {ex.Message}";
        }
    }

    [RelayCommand]
    private void Save()
    {
        try
        {
            LocalrefFfiMethods.SaveAppSettings(new AppSettings(
                configPath: ConfigPath,
                repoName: RepoName,
                libraryRoot: LibraryRoot,
                restAddr: RestAddr,
                restEndpoint: RestEndpoint,
                cscAddr: CscAddr,
                startHidden: StartHidden,
                quietStart: _quietStart));
            _startupService.SetEnabled(LaunchAtStartup, StartHidden);
            StatusText = "Settings saved. Restart Localref to apply changes.";
        }
        catch (Exception ex)
        {
            ExceptionService.Current.Report(ex, "Save settings", ExceptionSource.FFI);
            StatusText = $"Could not save settings: {ex.Message}";
        }
    }

    [RelayCommand]
    private void AddToStartMenu()
    {
        try
        {
            _startMenuService.AddShortcut();
            StatusText = "Localref was added to the Start menu.";
        }
        catch (Exception ex)
        {
            ExceptionService.Current.Report(ex, "Add Start menu shortcut", ExceptionSource.Command);
            StatusText = $"Could not add Localref to the Start menu: {ex.Message}";
        }
    }

    [RelayCommand]
    private async Task TestNotification()
    {
        if (_daemon is null)
        {
            return;
        }
        try
        {
            using var response = await HttpClient.PostAsJsonAsync(
                $"{_daemon.RestEndpoint.TrimEnd('/')}/api/notify",
                new { title = "Localref", body = "Windows notifications are working.", kind = "success" });
            StatusText = response.IsSuccessStatusCode
                ? "Test notification sent"
                : $"Notification service returned {(int)response.StatusCode}";
        }
        catch (Exception ex)
        {
            ExceptionService.Current.Report(ex, "Send test notification", ExceptionSource.Task);
            StatusText = $"Could not send test notification: {ex.Message}";
        }
    }
}
