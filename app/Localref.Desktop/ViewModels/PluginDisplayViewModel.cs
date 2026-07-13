using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.Linq;
using CommunityToolkit.Mvvm.ComponentModel;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>Host-rendered state for a schema-v2 plugin display surface.</summary>
public partial class PluginDisplayViewModel : ViewModelBase
{
    private readonly UiDisplay _display;

    public PluginDisplayViewModel(UiDisplay display)
    {
        _display = display;
        foreach (var column in display.columns)
        {
            Columns.Add(new PluginDisplayColumnViewModel(column));
        }
    }

    public string Id => _display.id;
    public string Title => _display.title ?? string.Empty;
    public string EmptyText => _display.emptyText ?? "No data available.";
    public string? SelectionField => _display.selectionField;
    public string? SelectionOf => _display.selectionOf;
    public bool IsText => _display.kind == DisplayKind.Text;
    public bool IsTable => _display.kind == DisplayKind.Table;
    public bool IsDetails => _display.kind == DisplayKind.Details;
    public bool IsStatus => _display.kind == DisplayKind.Status;
    public bool HasRows => Rows.Count > 0;
    public bool HasDetails => Details.Count > 0;

    public ObservableCollection<PluginDisplayColumnViewModel> Columns { get; } = new();
    public ObservableCollection<PluginDisplayRowViewModel> Rows { get; } = new();
    public ObservableCollection<PluginDisplayDetailViewModel> Details { get; } = new();

    [ObservableProperty]
    private string _text = "";

    [ObservableProperty]
    private string _errorText = "";

    [ObservableProperty]
    private PluginDisplayRowViewModel? _selectedRow;

    partial void OnSelectedRowChanged(PluginDisplayRowViewModel? value) =>
        SelectionChanged?.Invoke(this, value);

    public event EventHandler<PluginDisplayRowViewModel?>? SelectionChanged;

    public void SetText(string text)
    {
        Text = text;
        ErrorText = "";
    }

    public void SetError(string error)
    {
        ErrorText = error;
        Rows.Clear();
        Details.Clear();
        OnPropertyChanged(nameof(HasRows));
        OnPropertyChanged(nameof(HasDetails));
    }

    public void SetRows(IEnumerable<IReadOnlyDictionary<string, string>> rows)
    {
        Rows.Clear();
        foreach (var values in rows)
        {
            Rows.Add(new PluginDisplayRowViewModel(values, Columns));
        }
        ErrorText = "";
        OnPropertyChanged(nameof(HasRows));
    }

    public void SetDetails(IReadOnlyDictionary<string, string>? values)
    {
        Details.Clear();
        if (values is not null)
        {
            foreach (var column in Columns)
            {
                Details.Add(new PluginDisplayDetailViewModel(
                    column.Label,
                    values.TryGetValue(column.Key, out var value) ? value : "—"));
            }
        }
        OnPropertyChanged(nameof(HasDetails));
    }
}

public sealed class PluginDisplayColumnViewModel
{
    public PluginDisplayColumnViewModel(UiDisplayColumn column)
    {
        Key = column.key;
        Label = column.label;
    }

    public string Key { get; }
    public string Label { get; }
}

public sealed class PluginDisplayRowViewModel
{
    public PluginDisplayRowViewModel(
        IReadOnlyDictionary<string, string> values,
        IEnumerable<PluginDisplayColumnViewModel> columns)
    {
        Values = values;
        Cells = new ObservableCollection<string>(columns.Select(column =>
            values.TryGetValue(column.Key, out var value) ? value : "—"));
    }

    public IReadOnlyDictionary<string, string> Values { get; }
    public ObservableCollection<string> Cells { get; }
}

public sealed class PluginDisplayDetailViewModel
{
    public PluginDisplayDetailViewModel(string label, string value)
    {
        Label = label;
        Value = value;
    }

    public string Label { get; }
    public string Value { get; }
}
