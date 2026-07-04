using System.Collections.Generic;
using Localref.Desktop.Services;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>A plugin-owned page hosted in its own contextual window.</summary>
public sealed class PluginPageWindowViewModel
{
    public PluginPageWindowViewModel(
        DaemonService daemon,
        PluginDescriptor plugin,
        UiPage page,
        IReadOnlyList<string> selectedIds,
        string? activeId,
        string contextSummary)
    {
        PluginName = plugin.name;
        PageLabel = page.label;
        ContextSummary = contextSummary;
        Page = new PluginPageViewModel(daemon, plugin.name, page, selectedIds, activeId);
    }

    public string PluginName { get; }
    public string PageLabel { get; }
    public string ContextSummary { get; }
    public PluginPageViewModel Page { get; }
}
