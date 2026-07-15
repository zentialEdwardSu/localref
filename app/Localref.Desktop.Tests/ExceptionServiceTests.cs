using Localref.Desktop.Services;

namespace Localref.Desktop.Tests;

public sealed class ExceptionServiceTests
{
    [Fact]
    public void ClassifySeparatesCancellationRecoverableAndFatalExceptions()
    {
        using var service = CreateService();

        Assert.Equal(ExceptionDecision.Ignore, service.Classify(new OperationCanceledException()));
        Assert.Equal(ExceptionDecision.Continue, service.Classify(new InvalidOperationException("recoverable")));
        Assert.Equal(ExceptionDecision.Restart, service.Classify(new InvalidProgramException("fatal")));
    }

    [Fact]
    public void CancellationIsIgnoredWithoutJournalOrRestart()
    {
        using var service = CreateService();
        var restarted = false;
        service.RestartRequested += _ => restarted = true;

        var decision = service.Report(
            new OperationCanceledException(),
            "cancelled operation",
            ExceptionSource.Task);

        Assert.Equal(ExceptionDecision.Ignore, decision);
        Assert.Empty(service.RecentRecords);
        Assert.False(restarted);
    }

    [Fact]
    public void FatalReportRequestsNonZeroExitCode()
    {
        using var service = CreateService();
        RestartRequest? request = null;
        service.RestartRequested += value => request = value;

        var decision = service.Report(
            new InvalidProgramException("fatal"),
            "fatal operation",
            ExceptionSource.FFI);

        Assert.Equal(ExceptionDecision.Restart, decision);
        Assert.NotNull(request);
        Assert.NotEqual(0, request.ExitCode);
    }

    [Fact]
    public async Task RunAsyncCapturesFaultWithoutRethrowing()
    {
        using var service = CreateService();

        await service.RunAsync("async test", () => Task.FromException(new IOException("broken")));

        var record = Assert.Single(service.RecentRecords);
        Assert.Equal("async test", record.Operation);
        Assert.Contains("broken", record.Message);
    }

    [Fact]
    public async Task ObserveConsumesFaultedFireAndForgetTask()
    {
        using var service = CreateService();
        var reported = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        service.RecoverableException += _ => reported.TrySetResult();

        service.Observe(Task.FromException(new IOException("background")), "background test");
        await reported.Task.WaitAsync(TimeSpan.FromSeconds(2));

        Assert.Contains(service.RecentRecords, record => record.Operation == "background test");
    }

    [Fact]
    public void ThirdMatchingExceptionWithinWindowRequestsRestart()
    {
        using var service = CreateService();
        RestartRequest? request = null;
        service.RestartRequested += value => request = value;
        var exception = new InvalidOperationException("storm");

        service.Report(exception, "storm operation", ExceptionSource.UI);
        service.Report(exception, "storm operation", ExceptionSource.UI);
        var decision = service.Report(exception, "storm operation", ExceptionSource.UI);

        Assert.Equal(ExceptionDecision.Restart, decision);
        Assert.NotNull(request);
        Assert.NotEqual(0, request.ExitCode);
    }

    [Fact]
    public void SingleRecoverableExceptionKeepsCurrentProcess()
    {
        using var service = CreateService();
        var processId = Environment.ProcessId;
        var restartRequested = false;
        service.RestartRequested += _ => restartRequested = true;

        var decision = service.Report(
            new InvalidOperationException("ordinary UI failure"),
            "UI callback",
            ExceptionSource.UI);

        Assert.Equal(ExceptionDecision.Continue, decision);
        Assert.False(restartRequested);
        Assert.Equal(processId, Environment.ProcessId);
    }

    [Fact]
    public void StartupDeletesExceptionLogsOlderThanFourteenDays()
    {
        var root = Path.Combine(Path.GetTempPath(), "localref-exception-tests", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        var oldLog = Path.Combine(root, "exceptions-20000101.jsonl");
        File.WriteAllText(oldLog, "old");
        File.SetLastWriteTimeUtc(oldLog, DateTime.UtcNow.AddDays(-15));

        using var service = new ExceptionService(root);

        Assert.False(File.Exists(oldLog));
    }

    [Fact]
    public void UnwritableLogRootDoesNotBreakExceptionHandling()
    {
        var root = Path.Combine(Path.GetTempPath(), "localref-exception-tests", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(Path.GetDirectoryName(root)!);
        File.WriteAllText(root, "this is a file, not a directory");
        using var service = new ExceptionService(root);

        var exception = Record.Exception(() => service.Report(
            new IOException("write failure"),
            "unwritable journal",
            ExceptionSource.Task));

        Assert.Null(exception);
        Assert.Single(service.RecentRecords);
    }

    [Fact]
    public void FailingNotificationHandlerDoesNotRecurseOrEscape()
    {
        using var service = CreateService();
        service.RecoverableException += _ => throw new InvalidOperationException("notification failed");

        var exception = Record.Exception(() => service.Report(
            new IOException("operation failed"),
            "notification test",
            ExceptionSource.UI));

        Assert.Null(exception);
        Assert.Contains(service.RecentRecords, record => record.Operation == "notification test");
        Assert.Contains(service.RecentRecords, record => record.Operation == "Recoverable exception notification");
    }

    private static ExceptionService CreateService()
    {
        var root = Path.Combine(Path.GetTempPath(), "localref-exception-tests", Guid.NewGuid().ToString("N"));
        return new ExceptionService(root);
    }
}
