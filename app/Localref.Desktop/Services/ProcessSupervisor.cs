using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;

namespace Localref.Desktop.Services;

public static class ProcessSupervisor
{
    public const string ChildArgument = "--localref-supervised-child";
    private static readonly TimeSpan CrashWindow = TimeSpan.FromMinutes(5);
    private static readonly TimeSpan[] RestartDelays =
    [
        TimeSpan.FromSeconds(1),
        TimeSpan.FromSeconds(5),
        TimeSpan.FromSeconds(30),
    ];

    public static bool IsSupervisedChild(IReadOnlyList<string> args) =>
        args.Any(arg => string.Equals(arg, ChildArgument, StringComparison.Ordinal));

    public static bool IsDisabled => Debugger.IsAttached ||
        string.Equals(Environment.GetEnvironmentVariable("LOCALREF_DISABLE_SUPERVISOR"), "1", StringComparison.Ordinal);

    public static string[] ApplicationArguments(IEnumerable<string> args) =>
        args.Where(arg => !string.Equals(arg, ChildArgument, StringComparison.Ordinal)).ToArray();

    public static TimeSpan DelayForRestart(int restartCount) =>
        RestartDelays[Math.Clamp(restartCount - 1, 0, RestartDelays.Length - 1)];

    public static int Run(string[] originalArgs)
    {
        var executable = Environment.ProcessPath
            ?? throw new InvalidOperationException("Unable to locate the Localref executable.");
        return RunLoop(
            executable,
            originalArgs,
            new SystemProcessLauncher(),
            new SystemSupervisorClock(),
            ShowCrashLoopMessage);
    }

    public static int RunLoop(
        string executable,
        string[] originalArgs,
        ISupervisedProcessLauncher launcher,
        ISupervisorClock clock,
        Action? crashLoop = null)
    {
        var crashes = new Queue<DateTimeOffset>();

        while (true)
        {
            using var child = launcher.Start(CreateStartInfo(executable, originalArgs));
            child.WaitForExit();
            if (child.ExitCode == 0) return 0;

            var now = clock.UtcNow;
            while (crashes.TryPeek(out var timestamp) && now - timestamp > CrashWindow) crashes.Dequeue();
            crashes.Enqueue(now);
            var failure = $"Child process {child.Id} exited with code {child.ExitCode}.";
            WriteRecoveryMarker(failure);
            ExceptionService.Current.Report(
                new InvalidOperationException(failure),
                "Supervised child exit",
                ExceptionSource.Supervisor);

            if (crashes.Count > RestartDelays.Length)
            {
                ExceptionService.Current.Report(
                    new InvalidOperationException($"Localref entered a crash loop; last exit code {child.ExitCode}."),
                    "Supervisor crash loop", ExceptionSource.Supervisor, isTerminating: true);
                crashLoop?.Invoke();
                return child.ExitCode;
            }
            clock.Delay(DelayForRestart(crashes.Count));
        }
    }

    public static ProcessStartInfo CreateStartInfo(string executable, IEnumerable<string> args)
    {
        var startInfo = new ProcessStartInfo(executable)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        startInfo.ArgumentList.Add(ChildArgument);
        foreach (var arg in ApplicationArguments(args)) startInfo.ArgumentList.Add(arg);
        return startInfo;
    }

    public static void WriteRecoveryMarker(string reason)
    {
        try
        {
            var marker = RecoveryMarkerPath();
            Directory.CreateDirectory(Path.GetDirectoryName(marker)!);
            File.WriteAllText(marker, $"{DateTimeOffset.UtcNow:O}\n{reason}");
        }
        catch { }
    }

    public static bool ConsumeRecoveryMarker()
    {
        try
        {
            var marker = RecoveryMarkerPath();
            if (!File.Exists(marker)) return false;
            File.Delete(marker);
            return true;
        }
        catch { return false; }
    }

    private static string RecoveryMarkerPath() => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "Localref", "recovery.marker");

    private static void ShowCrashLoopMessage()
    {
        if (!OperatingSystem.IsWindows()) return;
        _ = MessageBox(IntPtr.Zero,
            "Localref stopped restarting because it failed repeatedly. See the exception log for details.",
            "Localref could not recover", 0x10);
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "MessageBoxW")]
    private static extern int MessageBox(IntPtr hWnd, string text, string caption, uint type);

    public interface ISupervisedProcess : IDisposable
    {
        int Id { get; }
        int ExitCode { get; }
        void WaitForExit();
    }

    public interface ISupervisedProcessLauncher
    {
        ISupervisedProcess Start(ProcessStartInfo startInfo);
    }

    public interface ISupervisorClock
    {
        DateTimeOffset UtcNow { get; }
        void Delay(TimeSpan delay);
    }

    private sealed class SystemProcessLauncher : ISupervisedProcessLauncher
    {
        public ISupervisedProcess Start(ProcessStartInfo startInfo) =>
            new SystemSupervisedProcess(Process.Start(startInfo)
                ?? throw new InvalidOperationException("Unable to start the supervised Localref process."));
    }

    private sealed class SystemSupervisedProcess(Process process) : ISupervisedProcess
    {
        public int Id => process.Id;
        public int ExitCode => process.ExitCode;
        public void WaitForExit() => process.WaitForExit();
        public void Dispose() => process.Dispose();
    }

    private sealed class SystemSupervisorClock : ISupervisorClock
    {
        public DateTimeOffset UtcNow => DateTimeOffset.UtcNow;
        public void Delay(TimeSpan delay) => Thread.Sleep(delay);
    }
}
