using Avalonia;
using System;
using Localref.Desktop.Services;

namespace Localref.Desktop;

sealed class Program
{
    // Initialization code. Don't use any Avalonia, third-party APIs or any
    // SynchronizationContext-reliant code before AppMain is called: things aren't initialized
    // yet and stuff might break.
    [STAThread]
    public static int Main(string[] args)
    {
        var exceptions = ExceptionService.Current;
        exceptions.InstallProcessHandlers();

        try
        {
            if (!ProcessSupervisor.IsSupervisedChild(args) && !ProcessSupervisor.IsDisabled)
            {
                return ProcessSupervisor.Run(args);
            }

            return BuildAvaloniaApp().StartWithClassicDesktopLifetime(
                ProcessSupervisor.ApplicationArguments(args));
        }
        catch (Exception exception)
        {
            exceptions.Report(
                exception,
                "Avalonia startup and main loop",
                ExceptionSource.Startup,
                isTerminating: true);
            return ExceptionService.ManagedFailureExitCode;
        }
    }

    // Avalonia configuration, don't remove; also used by visual designer.
    public static AppBuilder BuildAvaloniaApp()
        => AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .WithInterFont()
            .LogToTrace();
}
