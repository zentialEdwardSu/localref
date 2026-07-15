using Localref.Desktop.Services;

namespace Localref.Desktop.Tests;

public sealed class ProcessSupervisorTests
{
    [Fact]
    public void ChildStartPreservesApplicationArguments()
    {
        var startInfo = ProcessSupervisor.CreateStartInfo(
            "Localref.Desktop.exe",
            ["--silent", "--custom=value"]);

        Assert.Equal(
            [ProcessSupervisor.ChildArgument, "--silent", "--custom=value"],
            startInfo.ArgumentList);
    }

    [Fact]
    public void RestartBackoffUsesOneFiveThirtySeconds()
    {
        Assert.Equal(TimeSpan.FromSeconds(1), ProcessSupervisor.DelayForRestart(1));
        Assert.Equal(TimeSpan.FromSeconds(5), ProcessSupervisor.DelayForRestart(2));
        Assert.Equal(TimeSpan.FromSeconds(30), ProcessSupervisor.DelayForRestart(3));
        Assert.Equal(TimeSpan.FromSeconds(30), ProcessSupervisor.DelayForRestart(4));
    }

    [Fact]
    public void SupervisorSentinelIsNotPassedToAvalonia()
    {
        Assert.Equal(
            ["--silent"],
            ProcessSupervisor.ApplicationArguments(
                [ProcessSupervisor.ChildArgument, "--silent"]));
    }

    [Fact]
    public void SupervisionLoopRestartsFailuresAndStopsOnNormalExit()
    {
        var launcher = new FakeLauncher(70, -1, 0);
        var clock = new FakeClock();

        var exitCode = ProcessSupervisor.RunLoop(
            "Localref.Desktop.exe",
            ["--silent"],
            launcher,
            clock);

        Assert.Equal(0, exitCode);
        Assert.Equal(3, launcher.Starts.Count);
        Assert.All(launcher.Starts, start => Assert.Equal(
            [ProcessSupervisor.ChildArgument, "--silent"],
            start.ArgumentList));
        Assert.Equal(
            [TimeSpan.FromSeconds(1), TimeSpan.FromSeconds(5)],
            clock.Delays);
    }

    [Fact]
    public void SupervisionLoopStopsAfterFourthFailureInFiveMinutes()
    {
        var launcher = new FakeLauncher(70, 70, 70, 70);
        var clock = new FakeClock();
        var crashLoopShown = false;

        var exitCode = ProcessSupervisor.RunLoop(
            "Localref.Desktop.exe",
            [],
            launcher,
            clock,
            () => crashLoopShown = true);

        Assert.Equal(70, exitCode);
        Assert.True(crashLoopShown);
        Assert.Equal(4, launcher.Starts.Count);
        Assert.Equal(
            [TimeSpan.FromSeconds(1), TimeSpan.FromSeconds(5), TimeSpan.FromSeconds(30)],
            clock.Delays);
    }

    private sealed class FakeLauncher(params int[] exitCodes) : ProcessSupervisor.ISupervisedProcessLauncher
    {
        private readonly Queue<int> _exitCodes = new(exitCodes);
        public List<System.Diagnostics.ProcessStartInfo> Starts { get; } = new();

        public ProcessSupervisor.ISupervisedProcess Start(System.Diagnostics.ProcessStartInfo startInfo)
        {
            Starts.Add(startInfo);
            return new FakeProcess(Starts.Count, _exitCodes.Dequeue());
        }
    }

    private sealed class FakeProcess(int id, int exitCode) : ProcessSupervisor.ISupervisedProcess
    {
        public int Id { get; } = id;
        public int ExitCode { get; } = exitCode;
        public void WaitForExit() { }
        public void Dispose() { }
    }

    private sealed class FakeClock : ProcessSupervisor.ISupervisorClock
    {
        private DateTimeOffset _now = new(2026, 7, 13, 0, 0, 0, TimeSpan.Zero);
        public DateTimeOffset UtcNow => _now;
        public List<TimeSpan> Delays { get; } = new();

        public void Delay(TimeSpan delay)
        {
            Delays.Add(delay);
            _now += delay;
        }
    }
}
