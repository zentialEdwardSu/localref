using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Linq;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Localref.Desktop.Services;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>
/// Renders one plugin <see cref="UiPage"/>: builds a field per declared
/// <see cref="UiField"/>, collects their values into the `--param` form, runs
/// the action off the UI thread, and raises <see cref="SaveRequested"/> when the
/// result should be written to a file the user picks.
/// </summary>
public partial class PluginPageViewModel : ViewModelBase
{
    private readonly DaemonService _daemon;
    private readonly string _plugin;
    private readonly UiPage _page;
    private readonly IReadOnlyList<string> _selectedIds;
    private readonly string? _activeId;

    /// <summary>Fields the view renders.</summary>
    public ObservableCollection<PluginFieldViewModel> Fields { get; } = new();

    /// <summary>Schema-declared preview and display surfaces.</summary>
    public ObservableCollection<PluginDisplayViewModel> Displays { get; } = new();

    /// <summary>Page label / tab title.</summary>
    public string Label => _page.label;

    /// <summary>Whether this page has a submit action.</summary>
    public bool HasAction => _page.action is not null;

    public string RunLabel => _page.submit?.label ?? "Run";

    public bool CanRun => HasAction && Fields.All(candidate =>
        !candidate.Required || !string.IsNullOrWhiteSpace(candidate.Value));

    [ObservableProperty]
    private string _resultText = "";

    /// <summary>
    /// Raised when a successful action produced content to save. The view opens
    /// a save dialog and writes the content; the VM stays UI-toolkit-free.
    /// </summary>
    public event Func<string /*filename*/, string /*content*/, Task>? SaveRequested;

    /// <summary>Raised for a schema-declared confirmation immediately before submission.</summary>
    public event Func<UiConfirmation, string, Task<bool>>? ConfirmationRequested;

    public PluginPageViewModel(
        DaemonService daemon,
        string plugin,
        UiPage page,
        IReadOnlyList<string>? selectedIds = null,
        string? activeId = null)
    {
        _daemon = daemon;
        _plugin = plugin;
        _page = page;
        _selectedIds = selectedIds ?? Array.Empty<string>();
        _activeId = activeId;
        foreach (var field in page.fields)
        {
            var viewModel = new PluginFieldViewModel(field);
            viewModel.PropertyChanged += OnFieldChanged;
            Fields.Add(viewModel);
        }
        foreach (var display in page.display)
        {
            var viewModel = new PluginDisplayViewModel(display);
            viewModel.SelectionChanged += OnDisplaySelectionChanged;
            Displays.Add(viewModel);
        }
        _ = RefreshPreview();
    }

    private int _previewVersion;

    private void OnFieldChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(PluginFieldViewModel.Value))
        {
            OnPropertyChanged(nameof(CanRun));
            _ = RefreshPreview();
        }
    }

    private void OnDisplaySelectionChanged(object? sender, PluginDisplayRowViewModel? row)
    {
        if (sender is not PluginDisplayViewModel display || row is null)
        {
            return;
        }
        if (display.SelectionField is { } fieldName &&
            row.Values.TryGetValue(fieldName, out var value))
        {
            var field = Fields.FirstOrDefault(candidate => candidate.Name == fieldName);
            if (field is not null)
            {
                field.Value = value;
            }
        }
        foreach (var details in Displays.Where(candidate => candidate.SelectionOf == display.Id))
        {
            details.SetDetails(row.Values);
        }
    }

    /// <summary>Collect the current field values into the `--param` form map.</summary>
    private Dictionary<string, string> BuildForm()
    {
        var form = new Dictionary<string, string>();
        foreach (var field in Fields)
        {
            form[field.Name] = field.Value;
        }
        form["selected"] = string.Join(',', _selectedIds);
        if (!string.IsNullOrWhiteSpace(_activeId))
        {
            form["active"] = _activeId;
        }
        return form;
    }

    /// <summary>Debounce and load the page's optional structured preview.</summary>
    [RelayCommand]
    public async Task RefreshPreview()
    {
        if (_page.preview is not { } preview)
        {
            ApplyTierOneDisplays();
            return;
        }

        var version = Interlocked.Increment(ref _previewVersion);
        try
        {
            await Task.Delay(TimeSpan.FromMilliseconds(preview.debounceMs));
            var result = await Task.Run(() =>
                _daemon.Handle.PreviewPluginAction(_plugin, preview.action, BuildForm()));
            if (version != Volatile.Read(ref _previewVersion))
            {
                return;
            }
            if (result.status != "ok")
            {
                SetPreviewError(preview.into, result.message ?? "Preview failed.");
                return;
            }
            ApplyPreview(preview.into, result.result ?? "", result.contentType);
        }
        catch (Exception ex)
        {
            if (version == Volatile.Read(ref _previewVersion))
            {
                SetPreviewError(preview.into, ex.Message);
            }
        }
    }

    private void ApplyTierOneDisplays()
    {
        foreach (var display in Displays.Where(display => display.IsText || display.IsStatus))
        {
            display.SetText(ExpandTemplate(display.Text));
        }
    }

    private void ApplyPreview(string target, string payload, string? contentType)
    {
        ApplyTierOneDisplays();
        if (!string.Equals(contentType, "application/vnd.localref.plugin-ui+json;v=1", StringComparison.Ordinal))
        {
            Displays.FirstOrDefault(display => display.Id == target)?.SetText(payload);
            return;
        }

        try
        {
            using var document = JsonDocument.Parse(payload);
            if (document.RootElement.ValueKind != JsonValueKind.Object)
            {
                throw new JsonException("The structured preview must be a JSON object.");
            }
            foreach (var property in document.RootElement.EnumerateObject())
            {
                var display = Displays.FirstOrDefault(candidate => candidate.Id == property.Name);
                if (display is null)
                {
                    continue;
                }
                switch (display)
                {
                    case { IsTable: true }:
                        if (property.Value.ValueKind != JsonValueKind.Array)
                        {
                            throw new JsonException($"Display '{property.Name}' requires an array.");
                        }
                        display.SetRows(property.Value.EnumerateArray().Select(ReadRow));
                        break;
                    case { IsText: true } or { IsStatus: true }:
                        display.SetText(property.Value.ValueKind == JsonValueKind.String
                            ? property.Value.GetString() ?? ""
                            : property.Value.GetRawText());
                        break;
                }
            }
        }
        catch (JsonException ex)
        {
            SetPreviewError(target, $"Invalid plugin preview: {ex.Message}");
        }
    }

    private static IReadOnlyDictionary<string, string> ReadRow(JsonElement element)
    {
        if (element.ValueKind != JsonValueKind.Object)
        {
            throw new JsonException("Each table row must be an object.");
        }
        return element.EnumerateObject().ToDictionary(
            property => property.Name,
            property => property.Value.ValueKind == JsonValueKind.String
                ? property.Value.GetString() ?? ""
                : property.Value.GetRawText());
    }

    private void SetPreviewError(string id, string error) =>
        Displays.FirstOrDefault(display => display.Id == id)?.SetError(error);

    private string ExpandTemplate(string template)
    {
        var value = template.Replace("{selection.count}", _selectedIds.Count.ToString(), StringComparison.Ordinal);
        foreach (var field in Fields)
        {
            value = value.Replace($"{{field.{field.Name}}}", field.Value, StringComparison.Ordinal);
        }
        return value;
    }

    /// <summary>Run the page's action, then save or display the result.</summary>
    [RelayCommand]
    public async Task Run()
    {
        if (_page.action is not { } action)
        {
            return;
        }
        if (!CanRun)
        {
            ResultText = "Complete the required fields before running this action.";
            return;
        }
        if (_page.submit?.confirm is { } confirmation && ConfirmationRequested is { } confirm)
        {
            if (!await confirm(confirmation, ExpandTemplate(confirmation.message)))
            {
                return;
            }
        }
        var form = BuildForm();
        PluginRunResult result;
        try
        {
            // Blocks on the Rust side (spawns the plugin); keep it off the UI thread.
            result = await Task.Run(
                () => _daemon.Handle.RunPluginAction(_plugin, action, form));
        }
        catch (Exception ex)
        {
            ResultText = $"error: {ex.Message}";
            return;
        }

        if (result.status != "ok")
        {
            ResultText = result.message ?? "plugin action failed";
            return;
        }

        // Content + suggested filename → save dialog. Content only → inline.
        if (result.result is { } content)
        {
            if (result.filename is { } filename && SaveRequested is { } save)
            {
                await save(filename, content);
            }
            else
            {
                ResultText = content;
            }
        }
        else
        {
            ResultText = "done";
        }
        if (_page.submit?.refreshAfterSubmit == true)
        {
            await RefreshPreview();
        }
    }
}
