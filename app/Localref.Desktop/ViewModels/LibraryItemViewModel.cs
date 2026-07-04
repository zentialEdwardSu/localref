using System;
using System.Collections.Generic;
using System.Linq;
using Avalonia.Media;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>A display-friendly projection of an immutable FFI item record.</summary>
public sealed class LibraryItemViewModel(ItemDocument document) : ViewModelBase
{
    private bool _isSelected;

    public ItemDocument Document { get; } = document;

    public bool IsSelected
    {
        get => _isSelected;
        set => SetProperty(ref _isSelected, value);
    }

    public string Id => Document.id;
    public string Title => Document.title;
    public string Type => Humanize(Document.itemType);
    public string Authors => Document.authors.Length == 0
        ? "Unknown author"
        : string.Join(", ", Document.authors);
    public string Year => Document.year?.ToString() ?? "—";
    public string Venue => string.IsNullOrWhiteSpace(Document.venue) ? "Unspecified venue" : Document.venue;
    public string Doi => string.IsNullOrWhiteSpace(Document.doi) ? "No DOI" : Document.doi;
    public string Categories => Document.categories.Length == 0
        ? "Uncategorized"
        : string.Join(" · ", Document.categories);
    public IReadOnlyList<string> CategoryPaths => Document.categories.Length == 0
        ? ["Uncategorized"]
        : Document.categories;
    public string FileSummary
    {
        get
        {
            var count = Document.extraFiles.Length + (Document.mainFile is null ? 0 : 1);
            return count == 1 ? "1 file" : $"{count} files";
        }
    }

    /// <summary>
    /// Optional row accent brush a plugin can set via the reserved
    /// <c>ui.bar_color</c> extra (a CSS hex string like <c>#e11d48</c>); null
    /// when unset or unparseable, in which case no bar is shown.
    /// </summary>
    public IBrush? StatusBarBrush
    {
        get
        {
            if (!Document.extra.TryGetValue("ui", out var ui)
                || !ui.TryGetValue("bar_color", out var hex)
                || string.IsNullOrWhiteSpace(hex))
            {
                return null;
            }
            return Color.TryParse(hex, out var color) ? new SolidColorBrush(color) : null;
        }
    }

    private static string Humanize(string value) => string.Concat(
        value.Select((character, index) => index > 0 && char.IsUpper(character)
            ? $" {char.ToLowerInvariant(character)}"
            : character.ToString())).Replace('_', ' ');
}
