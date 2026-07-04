using System;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>File-row projection with explicit main-file state.</summary>
public sealed class ItemFileViewModel(ItemFileEntry entry, string? mainFile)
{
    public ItemFileEntry Entry { get; } = entry;
    public string Path => Entry.path;
    public string Kind => Entry.kind;
    public bool IsMain => string.Equals(Path, mainFile, StringComparison.OrdinalIgnoreCase);
    public bool IsMetadata => string.Equals(Path, "metadata.toml", StringComparison.OrdinalIgnoreCase);
    public bool CanBeMain => !IsMetadata;
    public string Role => IsMain ? "MAIN FILE" : IsMetadata ? "METADATA" : Kind;
    public string Size => Entry.bytes switch
    {
        null => "",
        < 1024 => $"{Entry.bytes} B",
        < 1024 * 1024 => $"{Entry.bytes / 1024d:0.#} KB",
        _ => $"{Entry.bytes / (1024d * 1024d):0.#} MB",
    };
}
