using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using uniffi.localref_ffi;

namespace Localref.Desktop.Services;

/// <summary>
/// Owns the process-wide <see cref="DaemonHandle"/> and the paths it was booted
/// with. Created once at startup (<see cref="Start"/>) and disposed on exit.
/// </summary>
/// <remarks>
/// The handle must outlive the app: it owns the Rust Tokio runtime and the REST
/// + CSC servers. Hold this in <c>App</c> and call <see cref="Stop"/> from the
/// desktop lifetime's exit path so the servers shut down cleanly.
/// </remarks>
public sealed class DaemonService : IPluginActionRunner
{
    private DaemonHandle? _handle;
    private CancellationTokenSource? _healthCancellation;
    private Task? _healthMonitor;

    /// <summary>The live daemon handle. Throws if accessed before <see cref="Start"/>.</summary>
    public DaemonHandle Handle =>
        _handle ?? throw new InvalidOperationException("daemon not started");

    /// <summary>
    /// True while the daemon is started and not yet stopped. Callbacks that may
    /// fire during teardown (e.g. late daemon events) should check this before
    /// touching <see cref="Handle"/>, which throws once <see cref="Stop"/> runs.
    /// </summary>
    public bool IsRunning => _handle is not null;

    /// <summary>The public REST endpoint plugins were told to call.</summary>
    public string RestEndpoint { get; private set; } = "";
    public string RepoName { get; private set; } = "Localref";
    public string LibraryRoot { get; private set; } = "";
    public string DaemonLogPath => Path.Combine(
        LibraryRoot, ".localref", "logs", "localref.jsonl");

    /// <summary>
    /// Boot the daemon using the shared on-disk configuration.
    /// </summary>
    /// <remarks>
    /// Configuration comes from the Rust core's <c>load_config()</c> — the same
    /// resolution the CLI uses (<c>LOCALREF_CONFIG</c> env var, else
    /// <c>~/.localref/config.toml</c>, creating it with defaults on first run).
    /// The desktop app no longer hardcodes ports or paths, so it and the CLI
    /// stay in sync.
    /// </remarks>
    public void Start()
    {
        var config = LocalrefFfiMethods.LoadConfig();
        var settings = LocalrefFfiMethods.LoadAppSettings();
        // The library + plugins directories must exist before the daemon opens
        // storage; config resolution does not create them.
        Directory.CreateDirectory(config.libraryRoot);
        Directory.CreateDirectory(config.pluginsDir);

        RestEndpoint = config.restEndpoint;
        RepoName = settings.repoName;
        LibraryRoot = config.libraryRoot;
        _handle = LocalrefFfiMethods.StartDaemon(config);
        _healthCancellation = new CancellationTokenSource();
        _healthMonitor = MonitorRuntimeHealthAsync(_handle, _healthCancellation.Token);
        ExceptionService.Current.Observe(
            _healthMonitor,
            "Monitor Rust runtime health",
            ExceptionSource.RustRuntime);
    }

    /// <summary>Stop subscriptions/servers. Safe to call more than once.</summary>
    public void Stop()
    {
        var cancellation = Interlocked.Exchange(ref _healthCancellation, null);
        cancellation?.Cancel();
        var monitor = Interlocked.Exchange(ref _healthMonitor, null);
        if (monitor is not null)
        {
            try { monitor.Wait(TimeSpan.FromSeconds(2)); }
            catch (AggregateException ex) when (ex.InnerExceptions.All(
                inner => inner is OperationCanceledException)) { }
            catch (Exception ex)
            {
                ExceptionService.Current.Report(
                    ex, "Stop Rust runtime health monitor", ExceptionSource.RustRuntime);
            }
        }
        cancellation?.Dispose();

        var handle = Interlocked.Exchange(ref _handle, null);
        if (handle is null) return;
        try
        {
            handle.Shutdown();
        }
        catch (Exception ex)
        {
            ExceptionService.Current.Report(ex, "Shutdown Rust daemon", ExceptionSource.FFI);
        }
        finally
        {
            try { handle.Dispose(); }
            catch (Exception ex)
            {
                ExceptionService.Current.Report(ex, "Dispose Rust daemon", ExceptionSource.FFI);
            }
        }
    }

    private static async Task MonitorRuntimeHealthAsync(
        DaemonHandle handle,
        CancellationToken cancellationToken)
    {
        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(2));
        while (await timer.WaitForNextTickAsync(cancellationToken).ConfigureAwait(false))
        {
            var health = handle.RuntimeHealth();
            if (health.state == RuntimeHealthState.Fatal)
            {
                ExceptionService.Current.Report(
                    new InvalidOperationException($"{health.component}: {health.message}"),
                    "Rust runtime reported fatal health",
                    ExceptionSource.RustRuntime);
                return;
            }
        }
    }

    public PluginRunResult PreviewPluginAction(
        string plugin,
        string action,
        Dictionary<string, string> form) =>
        Handle.PreviewPluginAction(plugin, action, form);

    public PluginRunResult RunPluginAction(
        string plugin,
        string action,
        Dictionary<string, string> form) =>
        Handle.RunPluginAction(plugin, action, form);
}
