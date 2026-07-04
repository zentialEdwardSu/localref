using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
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

    /// <summary>Page label / tab title.</summary>
    public string Label => _page.label;

    /// <summary>Whether this page has a submit action.</summary>
    public bool HasAction => _page.action is not null;

    [ObservableProperty]
    private string _resultText = "";

    /// <summary>
    /// Raised when a successful action produced content to save. The view opens
    /// a save dialog and writes the content; the VM stays UI-toolkit-free.
    /// </summary>
    public event Func<string /*filename*/, string /*content*/, Task>? SaveRequested;

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
            Fields.Add(new PluginFieldViewModel(field));
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

    /// <summary>Run the page's action, then save or display the result.</summary>
    [RelayCommand]
    public async Task Run()
    {
        if (_page.action is not { } action)
        {
            return;
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
    }
}
