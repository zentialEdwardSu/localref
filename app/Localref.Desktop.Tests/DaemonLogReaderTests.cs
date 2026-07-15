using System.Text;
using Localref.Desktop.Services;

namespace Localref.Desktop.Tests;

public sealed class DaemonLogReaderTests
{
    [Fact]
    public async Task ReadsExactJsonlWhileDaemonFileIsOpen()
    {
        var directory = Path.Combine(Path.GetTempPath(), $"localref-log-{Guid.NewGuid():N}");
        Directory.CreateDirectory(directory);
        var path = Path.Combine(directory, "localref.jsonl");
        const string expected = "{\"level\":\"INFO\",\"message\":\"first\"}\n" +
                                "{\"level\":\"ERROR\",\"message\":\"second\"}\n";

        try
        {
            await using var daemonWriter = new FileStream(
                path,
                FileMode.Create,
                FileAccess.Write,
                FileShare.ReadWrite | FileShare.Delete);
            var bytes = Encoding.UTF8.GetBytes(expected);
            await daemonWriter.WriteAsync(bytes);
            await daemonWriter.FlushAsync();

            var actual = await DaemonLogReader.ReadAllTextAsync(path);

            Assert.Equal(expected, actual);
        }
        finally
        {
            Directory.Delete(directory, recursive: true);
        }
    }

    [Fact]
    public async Task MissingLogReturnsNull()
    {
        var path = Path.Combine(Path.GetTempPath(), $"missing-localref-log-{Guid.NewGuid():N}.jsonl");

        Assert.Null(await DaemonLogReader.ReadAllTextAsync(path));
    }

    [Fact]
    public async Task ReadAsyncFormatsFieldsAndPrettyPrintsNestedJsonMessage()
    {
        var path = Path.Combine(Path.GetTempPath(), $"localref-log-{Guid.NewGuid():N}.jsonl");
        const string jsonl = "{\"id\":10,\"ts\":\"2026-06-23T03:08:36.267Z\"," +
                             "\"level\":\"INFO\",\"target\":\"localref::csc_event\"," +
                             "\"message\":\"{\\\"kind\\\":\\\"method_call\\\",\\\"id\\\":7}\"}\n";
        try
        {
            await File.WriteAllTextAsync(path, jsonl, Encoding.UTF8);

            var result = await DaemonLogReader.ReadAsync(path);

            Assert.NotNull(result);
            Assert.Equal(1, result.EntryCount);
            Assert.Equal(0, result.InvalidLineCount);
            Assert.Contains("2026-06-23 03:08:36.267Z  INFO   csc_event", result.DisplayText);
            Assert.Contains("  \"kind\": \"method_call\"", result.DisplayText);
            Assert.DoesNotContain("\\\"kind\\\"", result.DisplayText);
        }
        finally
        {
            File.Delete(path);
        }
    }

    [Fact]
    public async Task ReadAsyncPreservesMalformedLinesWithMarker()
    {
        var path = Path.Combine(Path.GetTempPath(), $"localref-log-{Guid.NewGuid():N}.jsonl");
        try
        {
            await File.WriteAllTextAsync(path, "not-json\n[]\n", Encoding.UTF8);

            var result = await DaemonLogReader.ReadAsync(path);

            Assert.NotNull(result);
            Assert.Equal(2, result.InvalidLineCount);
            Assert.Contains("[unparsed] not-json", result.DisplayText);
        }
        finally
        {
            File.Delete(path);
        }
    }
}
