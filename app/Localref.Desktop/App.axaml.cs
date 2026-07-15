using System;
using System.Linq;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using Avalonia.Threading;
using Localref.Desktop.Services;
using Localref.Desktop.ViewModels;
using Localref.Desktop.Views;

namespace Localref.Desktop;

public partial class App : Application
{
    /// <summary>Process-wide daemon owner; started here, stopped on exit.</summary>
    public DaemonService Daemon { get; } = new();

    private Window? _mainWindow;
    private bool _isShuttingDown;
    private IClassicDesktopStyleApplicationLifetime? _desktop;

    public override void Initialize()
    {
        AvaloniaXamlLoader.Load(this);
    }

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            _desktop = desktop;
            desktop.ShutdownMode = ShutdownMode.OnExplicitShutdown;
            ExceptionService.Current.RestartRequested += OnRestartRequested;
            ExceptionService.Current.RecoverableException += OnRecoverableException;
            Dispatcher.UIThread.UnhandledException += (_, args) =>
                ExceptionService.Current.HandleDispatcherException(args);
            WindowsNotificationIdentity.Register();

            try
            {
                // Boot the Rust daemon (Tokio runtime + REST + CSC + workers)
                // before the first view model touches it. Shut it down on exit
                // so the servers release their ports. A failure here (e.g. a
                // port already held by another instance, or an unreadable
                // config) must surface as an error window, not a silent hard
                // crash before any UI exists.
                Daemon.Start();
                desktop.ShutdownRequested += (_, _) => _isShuttingDown = true;
                desktop.Exit += (_, _) =>
                {
                    ExceptionService.Current.RestartRequested -= OnRestartRequested;
                    ExceptionService.Current.RecoverableException -= OnRecoverableException;
                    Daemon.Stop();
                };

                var viewModel = new MainWindowViewModel(Daemon);
                _mainWindow = new MainWindow
                {
                    DataContext = viewModel,
                };
                if (ProcessSupervisor.ConsumeRecoveryMarker())
                {
                    viewModel.StatusText = "Localref 已从异常中恢复，可查看日志";
                }
                _mainWindow.Closing += OnMainWindowClosing;

                if (!ShouldStartHidden(desktop))
                {
                    desktop.MainWindow = _mainWindow;
                }
            }
            catch (Exception ex)
            {
                var decision = ExceptionService.Current.Report(
                    ex, "Start desktop daemon", ExceptionSource.Startup);
                if (decision == ExceptionDecision.Restart)
                {
                    ExceptionService.Current.RequestRestart(
                        $"Desktop startup failed: {ex.Message}",
                        ex is uniffi.localref_ffi.PanicException
                            ? ExceptionService.RustFailureExitCode
                            : ExceptionService.ManagedFailureExitCode);
                }
                else
                {
                    ShowStartupError(desktop, ex);
                }
            }
        }

        base.OnFrameworkInitializationCompleted();
    }

    /// Show a minimal error window when the daemon fails to boot, so the user
    /// sees why instead of the process vanishing. Closing it exits the app.
    private static void ShowStartupError(
        IClassicDesktopStyleApplicationLifetime desktop,
        Exception ex)
    {
        var window = new Window
        {
            Title = "Localref could not start",
            Width = 460,
            Height = 200,
            WindowStartupLocation = WindowStartupLocation.CenterScreen,
            Content = new TextBlock
            {
                Margin = new Thickness(24),
                TextWrapping = Avalonia.Media.TextWrapping.Wrap,
                Text =
                    "Localref could not start its background service:\n\n" +
                    ex.Message +
                    "\n\nAnother Localref instance may already be running, or " +
                    "its configured ports may be in use.",
            },
        };
        // Closing the error window should end the process now that the daemon
        // never came up.
        desktop.ShutdownMode = ShutdownMode.OnMainWindowClose;
        desktop.MainWindow = window;
        window.Show();
    }

    private void OnTrayClicked(object? sender, EventArgs e) => ShowMainWindow();

    private void OnTrayOpen(object? sender, EventArgs e) => ShowMainWindow();

    private void OnTrayScan(object? sender, EventArgs e)
    {
        try { Daemon.Handle.ScanAll(); }
        catch (Exception ex)
        {
            ExceptionService.Current.Report(ex, "Tray scan", ExceptionSource.Command);
        }
    }

    private void OnTrayQuit(object? sender, EventArgs e)
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            _isShuttingDown = true;
            desktop.Shutdown();
        }
    }

    private void OnMainWindowClosing(object? sender, WindowClosingEventArgs e)
    {
        if (_isShuttingDown || sender is not Window window)
        {
            return;
        }

        e.Cancel = true;
        window.Hide();
    }

    private static bool ShouldStartHidden(IClassicDesktopStyleApplicationLifetime desktop)
    {
        if (desktop.Args?.Any(arg => string.Equals(arg, "--silent", StringComparison.OrdinalIgnoreCase)) == true)
        {
            return true;
        }

        try
        {
            return uniffi.localref_ffi.LocalrefFfiMethods.LoadAppSettings().startHidden;
        }
        catch (Exception ex)
        {
            ExceptionService.Current.Report(ex, "Read hidden-start setting", ExceptionSource.Startup);
            return false;
        }
    }

    private void ShowMainWindow()
    {
        if (_mainWindow is null)
        {
            return;
        }
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            desktop.MainWindow ??= _mainWindow;
        }
        _mainWindow.Show();
        _mainWindow.WindowState = WindowState.Normal;
        _mainWindow.Activate();
    }

    private void OnRecoverableException(ExceptionRecord record)
    {
        Dispatcher.UIThread.Post(() =>
        {
            if (_mainWindow?.DataContext is MainWindowViewModel viewModel)
            {
                viewModel.StatusText = $"当前操作已终止，Localref 可继续使用：{record.Message}";
            }
        });
    }

    private void OnRestartRequested(RestartRequest request)
    {
        void ShutdownForRestart()
        {
            if (_desktop is null || _isShuttingDown) return;
            _isShuttingDown = true;
            try
            {
                Daemon.Stop();
                _desktop.Shutdown(request.ExitCode);
            }
            catch (Exception ex)
            {
                ExceptionService.Current.EmergencyReport(
                    ex, "Controlled restart shutdown", ExceptionSource.AppDomain, true);
                Environment.Exit(request.ExitCode);
            }
        }

        if (Dispatcher.UIThread.CheckAccess())
        {
            ShutdownForRestart();
            return;
        }

        try { Dispatcher.UIThread.Post(ShutdownForRestart); }
        catch (Exception ex)
        {
            ExceptionService.Current.EmergencyReport(
                ex, "Queue controlled restart", ExceptionSource.AppDomain, true);
            Environment.Exit(request.ExitCode);
        }
    }
}
