using System;
using System.Linq;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
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

    public override void Initialize()
    {
        AvaloniaXamlLoader.Load(this);
    }

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            desktop.ShutdownMode = ShutdownMode.OnExplicitShutdown;
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
                desktop.Exit += (_, _) => Daemon.Stop();

                _mainWindow = new MainWindow
                {
                    DataContext = new MainWindowViewModel(Daemon),
                };
                _mainWindow.Closing += OnMainWindowClosing;

                if (!ShouldStartHidden(desktop))
                {
                    desktop.MainWindow = _mainWindow;
                }
            }
            catch (Exception ex)
            {
                ShowStartupError(desktop, ex);
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
        catch (Exception) { /* surfaced in the log pane on next refresh */ }
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
        catch
        {
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
}
