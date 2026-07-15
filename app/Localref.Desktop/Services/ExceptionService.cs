using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using Avalonia.Threading;
using uniffi.localref_ffi;

namespace Localref.Desktop.Services;

public enum ExceptionSource { Startup, UI, Command, Task, FFI, RustRuntime, AppDomain, Supervisor }
public enum ExceptionDecision { Ignore, Continue, Restart }

public sealed record ExceptionRecord(
    DateTimeOffset UtcTime,
    ExceptionSource Source,
    string Operation,
    string ExceptionType,
    string Message,
    string StackTrace,
    int ManagedThreadId,
    int ProcessId,
    string ApplicationVersion,
    string Fingerprint,
    bool IsTerminating,
    int RestartCount);

public sealed record RestartRequest(string Reason, int ExitCode);

/// <summary>Process-wide exception classifier, journal, and restart gateway.</summary>
public sealed class ExceptionService : IDisposable
{
    public const int ManagedFailureExitCode = 70;
    public const int RustFailureExitCode = 71;

    private static readonly Lazy<ExceptionService> Shared = new(() => new ExceptionService());
    private static readonly TimeSpan StormWindow = TimeSpan.FromSeconds(30);
    private static readonly TimeSpan NotificationWindow = TimeSpan.FromSeconds(10);
    private readonly object _stateGate = new();
    private readonly object _emergencyGate = new();
    private readonly Dictionary<string, Queue<DateTimeOffset>> _occurrences = new(StringComparer.Ordinal);
    private readonly Dictionary<string, DateTimeOffset> _notifications = new(StringComparer.Ordinal);
    private readonly ConcurrentQueue<ExceptionRecord> _recent = new();
    private readonly Channel<ExceptionRecord> _records;
    private readonly CancellationTokenSource _writerCancellation = new();
    private readonly string _logRoot;
    private readonly Task _writer;
    private FileStream? _emergencyStream;
    private int _processHandlersInstalled;
    private int _dispatcherHandlerActive;
    private int _restartRequested;
    private int _disposed;

    public static ExceptionService Current => Shared.Value;
    public event Action<ExceptionRecord>? RecoverableException;
    public event Action<RestartRequest>? RestartRequested;

    public ExceptionService(string? logRoot = null)
    {
        _logRoot = logRoot ?? Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Localref", "logs");
        _records = Channel.CreateUnbounded<ExceptionRecord>(new UnboundedChannelOptions
        {
            SingleReader = true,
            SingleWriter = false,
            AllowSynchronousContinuations = false,
        });
        PrepareLogDirectory();
        _writer = Task.Run(WriteLoopAsync);
    }

    public IReadOnlyList<ExceptionRecord> RecentRecords => _recent.ToArray();

    public void InstallProcessHandlers()
    {
        if (Interlocked.Exchange(ref _processHandlersInstalled, 1) != 0) return;

        AppDomain.CurrentDomain.UnhandledException += (_, args) =>
        {
            var exception = args.ExceptionObject as Exception
                ?? new Exception($"Unhandled non-Exception object: {args.ExceptionObject}");
            EmergencyReport(exception, "AppDomain.UnhandledException", ExceptionSource.AppDomain, args.IsTerminating);
        };
        TaskScheduler.UnobservedTaskException += (_, args) =>
        {
            Report(args.Exception, "TaskScheduler.UnobservedTaskException", ExceptionSource.Task);
            args.SetObserved();
        };
    }

    public void Run(string operation, Action action, ExceptionSource source = ExceptionSource.Command)
    {
        try { action(); }
        catch (Exception exception) { EscalateIfRequired(Report(exception, operation, source), exception, operation); }
    }

    public T? Run<T>(string operation, Func<T> action, ExceptionSource source = ExceptionSource.Command)
    {
        try { return action(); }
        catch (Exception exception)
        {
            EscalateIfRequired(Report(exception, operation, source), exception, operation);
            return default;
        }
    }

    public async Task RunAsync(string operation, Func<Task> action, ExceptionSource source = ExceptionSource.Task)
    {
        try { await action().ConfigureAwait(true); }
        catch (Exception exception) { EscalateIfRequired(Report(exception, operation, source), exception, operation); }
    }

    public async Task<T?> RunAsync<T>(string operation, Func<Task<T>> action, ExceptionSource source = ExceptionSource.Task)
    {
        try { return await action().ConfigureAwait(true); }
        catch (Exception exception)
        {
            EscalateIfRequired(Report(exception, operation, source), exception, operation);
            return default;
        }
    }

    public void Observe(Task task, string operation, ExceptionSource source = ExceptionSource.Task)
    {
        ArgumentNullException.ThrowIfNull(task);
        _ = ObserveCoreAsync(task, operation, source);
    }

    public ExceptionDecision Report(Exception exception, string operation, ExceptionSource source, bool isTerminating = false)
    {
        ArgumentNullException.ThrowIfNull(exception);
        var effective = Unwrap(exception);
        var now = DateTimeOffset.UtcNow;
        var fingerprint = CreateFingerprint(effective, operation);
        var decision = Classify(effective);
        if (decision == ExceptionDecision.Ignore) return decision;
        var occurrenceCount = RegisterOccurrence(fingerprint, now);
        if (decision == ExceptionDecision.Continue && occurrenceCount >= 3) decision = ExceptionDecision.Restart;

        var record = CreateRecord(effective, operation, source, fingerprint, isTerminating, occurrenceCount);
        Remember(record);
        _records.Writer.TryWrite(record);
        if (decision == ExceptionDecision.Continue && ShouldNotify(fingerprint, now))
        {
            try { RecoverableException?.Invoke(record); }
            catch (Exception notificationFailure)
            {
                EmergencyReport(notificationFailure, "Recoverable exception notification", ExceptionSource.UI, false);
            }
        }
        if (decision == ExceptionDecision.Restart)
        {
            var exitCode = effective is PanicException || source == ExceptionSource.RustRuntime
                ? RustFailureExitCode
                : ManagedFailureExitCode;
            RequestRestart($"{operation}: {effective.GetType().Name}: {effective.Message}", exitCode);
        }
        return decision;
    }

    public void HandleDispatcherException(DispatcherUnhandledExceptionEventArgs args)
    {
        if (Interlocked.Exchange(ref _dispatcherHandlerActive, 1) != 0)
        {
            args.Handled = false;
            return;
        }
        try
        {
            var decision = Report(args.Exception, "Dispatcher.UIThread", ExceptionSource.UI);
            args.Handled = true;
            EscalateIfRequired(decision, args.Exception, "Dispatcher.UIThread");
        }
        catch (Exception handlerFailure)
        {
            EmergencyReport(handlerFailure, "Dispatcher exception handler", ExceptionSource.AppDomain, true);
            args.Handled = false;
        }
        finally { Volatile.Write(ref _dispatcherHandlerActive, 0); }
    }

    public ExceptionDecision Classify(Exception exception)
    {
        var effective = Unwrap(exception);
        if (effective is OperationCanceledException) return ExceptionDecision.Ignore;
        return effective is PanicException
            or OutOfMemoryException
            or InsufficientMemoryException
            or AccessViolationException
            or SEHException
            or InvalidProgramException
            or StackOverflowException
            ? ExceptionDecision.Restart
            : ExceptionDecision.Continue;
    }

    public void RequestRestart(string reason, int exitCode = ManagedFailureExitCode)
    {
        if (exitCode == 0) throw new ArgumentOutOfRangeException(nameof(exitCode));
        if (Interlocked.Exchange(ref _restartRequested, 1) != 0) return;
        ProcessSupervisor.WriteRecoveryMarker(reason);
        try { RestartRequested?.Invoke(new RestartRequest(reason, exitCode)); }
        catch (Exception restartFailure)
        {
            EmergencyReport(restartFailure, "Restart request handler", ExceptionSource.AppDomain, true);
        }
    }

    public void EmergencyReport(Exception exception, string operation, ExceptionSource source, bool isTerminating)
    {
        if (!Monitor.TryEnter(_emergencyGate)) return;
        try
        {
            var effective = Unwrap(exception);
            var record = CreateRecord(effective, operation, source, CreateFingerprint(effective, operation), isTerminating, 0);
            Remember(record);
            if (_emergencyStream is not null)
            {
                var bytes = Encoding.UTF8.GetBytes(JsonSerializer.Serialize(record) + Environment.NewLine);
                _emergencyStream.Write(bytes);
                _emergencyStream.Flush(flushToDisk: true);
            }
        }
        catch { }
        finally { Monitor.Exit(_emergencyGate); }
    }

    public void Dispose()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0) return;
        _records.Writer.TryComplete();
        _writerCancellation.CancelAfter(TimeSpan.FromSeconds(1));
        try { _writer.GetAwaiter().GetResult(); } catch { }
        _writerCancellation.Dispose();
        _emergencyStream?.Dispose();
    }

    private async Task ObserveCoreAsync(Task task, string operation, ExceptionSource source)
    {
        try { await task.ConfigureAwait(false); }
        catch (Exception exception) { EscalateIfRequired(Report(exception, operation, source), exception, operation); }
    }

    private void EscalateIfRequired(ExceptionDecision decision, Exception exception, string operation)
    {
        if (decision != ExceptionDecision.Restart) return;
        var exitCode = Unwrap(exception) is PanicException ? RustFailureExitCode : ManagedFailureExitCode;
        RequestRestart($"{operation}: {exception.GetType().Name}: {exception.Message}", exitCode);
    }

    private int RegisterOccurrence(string fingerprint, DateTimeOffset now)
    {
        lock (_stateGate)
        {
            if (!_occurrences.TryGetValue(fingerprint, out var occurrences))
            {
                occurrences = new Queue<DateTimeOffset>();
                _occurrences[fingerprint] = occurrences;
            }
            while (occurrences.TryPeek(out var timestamp) && now - timestamp > StormWindow) occurrences.Dequeue();
            occurrences.Enqueue(now);
            return occurrences.Count;
        }
    }

    private bool ShouldNotify(string fingerprint, DateTimeOffset now)
    {
        lock (_stateGate)
        {
            if (_notifications.TryGetValue(fingerprint, out var previous) && now - previous < NotificationWindow) return false;
            _notifications[fingerprint] = now;
            return true;
        }
    }

    private void PrepareLogDirectory()
    {
        try
        {
            Directory.CreateDirectory(_logRoot);
            var cutoff = DateTime.UtcNow.Date.AddDays(-14);
            foreach (var file in Directory.EnumerateFiles(_logRoot, "exceptions-*.jsonl"))
            {
                try { if (File.GetLastWriteTimeUtc(file) < cutoff) File.Delete(file); } catch { }
            }
            _emergencyStream = new FileStream(LogPath(DateTimeOffset.UtcNow), FileMode.Append, FileAccess.Write,
                FileShare.ReadWrite, 4096, FileOptions.WriteThrough);
        }
        catch { _emergencyStream = null; }
    }

    private async Task WriteLoopAsync()
    {
        try
        {
            await foreach (var record in _records.Reader.ReadAllAsync(_writerCancellation.Token).ConfigureAwait(false))
            {
                try
                {
                    Directory.CreateDirectory(_logRoot);
                    await File.AppendAllTextAsync(LogPath(record.UtcTime), JsonSerializer.Serialize(record) + Environment.NewLine,
                        Encoding.UTF8, _writerCancellation.Token).ConfigureAwait(false);
                }
                catch (OperationCanceledException) when (_writerCancellation.IsCancellationRequested) { return; }
                catch { }
            }
        }
        catch (OperationCanceledException) when (_writerCancellation.IsCancellationRequested) { }
    }

    private string LogPath(DateTimeOffset timestamp) => Path.Combine(_logRoot, $"exceptions-{timestamp.UtcDateTime:yyyyMMdd}.jsonl");

    private void Remember(ExceptionRecord record)
    {
        _recent.Enqueue(record);
        while (_recent.Count > 200) _recent.TryDequeue(out _);
    }

    private static ExceptionRecord CreateRecord(Exception exception, string operation, ExceptionSource source,
        string fingerprint, bool terminating, int restartCount) => new(
            DateTimeOffset.UtcNow, source, operation,
            exception.GetType().FullName ?? exception.GetType().Name,
            exception.Message, exception.ToString(), Environment.CurrentManagedThreadId,
            Environment.ProcessId, Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "unknown",
            fingerprint, terminating, restartCount);

    private static Exception Unwrap(Exception exception)
    {
        while (exception is AggregateException { InnerExceptions.Count: 1 } aggregate) exception = aggregate.InnerExceptions[0];
        return exception;
    }

    private static string CreateFingerprint(Exception exception, string operation)
    {
        var firstFrame = exception.StackTrace?.Split('\n', StringSplitOptions.RemoveEmptyEntries).FirstOrDefault()?.Trim() ?? "";
        var input = $"{exception.GetType().FullName}|{operation}|{firstFrame}";
        return Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(input)))[..16];
    }
}
