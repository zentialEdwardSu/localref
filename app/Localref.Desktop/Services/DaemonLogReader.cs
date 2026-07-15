using System;
using System.Collections.Generic;
using System.IO;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;

namespace Localref.Desktop.Services;

/// <summary>Reads the daemon JSONL while the Rust appender is still writing it.</summary>
public static class DaemonLogReader
{
    private static readonly JsonSerializerOptions PrettyJson = new()
    {
        WriteIndented = true,
    };

    public sealed record ReadResult(
        string DisplayText,
        int EntryCount,
        long SourceBytes,
        int InvalidLineCount);

    public static async Task<ReadResult?> ReadAsync(
        string path,
        CancellationToken cancellationToken = default)
    {
        var raw = await ReadAllTextAsync(path, cancellationToken).ConfigureAwait(false);
        return raw is null ? null : Format(raw, Encoding.UTF8.GetByteCount(raw));
    }

    public static async Task<string?> ReadAllTextAsync(
        string path,
        CancellationToken cancellationToken = default)
    {
        if (!File.Exists(path))
        {
            return null;
        }

        await using var stream = new FileStream(
            path,
            FileMode.Open,
            FileAccess.Read,
            FileShare.ReadWrite | FileShare.Delete,
            bufferSize: 64 * 1024,
            FileOptions.Asynchronous | FileOptions.SequentialScan);
        using var reader = new StreamReader(
            stream,
            Encoding.UTF8,
            detectEncodingFromByteOrderMarks: true,
            bufferSize: 64 * 1024,
            leaveOpen: false);
        return await reader.ReadToEndAsync(cancellationToken).ConfigureAwait(false);
    }

    internal static ReadResult Format(string jsonl, long sourceBytes)
    {
        var output = new StringBuilder(jsonl.Length);
        var entryCount = 0;
        var invalidLineCount = 0;

        using var lines = new StringReader(jsonl);
        while (lines.ReadLine() is { } line)
        {
            if (string.IsNullOrWhiteSpace(line))
            {
                continue;
            }

            if (entryCount > 0)
            {
                output.AppendLine();
            }
            entryCount++;
            try
            {
                using var document = JsonDocument.Parse(line);
                if (document.RootElement.ValueKind == JsonValueKind.Object)
                {
                    AppendEntry(output, document.RootElement);
                }
                else
                {
                    invalidLineCount++;
                    output.Append("[unparsed] ").AppendLine(line);
                }
            }
            catch (JsonException)
            {
                invalidLineCount++;
                output.Append("[unparsed] ").AppendLine(line);
            }
        }

        return new ReadResult(output.ToString(), entryCount, sourceBytes, invalidLineCount);
    }

    private static void AppendEntry(StringBuilder output, JsonElement entry)
    {
        var timestamp = GetString(entry, "ts");
        var level = GetString(entry, "level");
        var target = GetString(entry, "target").Replace("localref::", "", StringComparison.Ordinal);
        var message = GetString(entry, "message");

        output.Append(FormatTimestamp(timestamp))
            .Append("  ")
            .Append(level.PadRight(5))
            .Append("  ")
            .Append(target);

        if (TryPrettyJson(message, out var prettyMessage))
        {
            output.AppendLine();
            foreach (var line in prettyMessage.Split('\n'))
            {
                output.Append("    ").AppendLine(line.TrimEnd('\r'));
            }
        }
        else if (!string.IsNullOrEmpty(message))
        {
            output.Append("  ").Append(message);
        }

        var metadata = new List<string>(3);
        AddMetadata(entry, metadata, "event_kind", "event");
        AddMetadata(entry, metadata, "item_id", "item");
        AddMetadata(entry, metadata, "path", "path");
        if (metadata.Count > 0)
        {
            output.AppendLine();
            output.Append("    ").Append(string.Join("  ", metadata));
        }
    }

    private static bool TryPrettyJson(string value, out string pretty)
    {
        pretty = "";
        var trimmed = value.Trim();
        if (trimmed.Length < 2 || (trimmed[0] != '{' && trimmed[0] != '['))
        {
            return false;
        }
        try
        {
            using var nested = JsonDocument.Parse(trimmed);
            pretty = JsonSerializer.Serialize(nested.RootElement, PrettyJson);
            return true;
        }
        catch (JsonException)
        {
            return false;
        }
    }

    private static string GetString(JsonElement element, string propertyName) =>
        element.TryGetProperty(propertyName, out var value) && value.ValueKind == JsonValueKind.String
            ? value.GetString() ?? ""
            : "";

    private static string FormatTimestamp(string timestamp) =>
        DateTimeOffset.TryParse(timestamp, out var parsed)
            ? parsed.UtcDateTime.ToString("yyyy-MM-dd HH:mm:ss.fff'Z'")
            : timestamp;

    private static void AddMetadata(
        JsonElement entry,
        ICollection<string> metadata,
        string propertyName,
        string label)
    {
        var value = GetString(entry, propertyName);
        if (!string.IsNullOrWhiteSpace(value))
        {
            metadata.Add($"{label}={value}");
        }
    }
}
