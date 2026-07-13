using System.Collections.Generic;
using uniffi.localref_ffi;

namespace Localref.Desktop.Services;

/// <summary>Runs plugin actions for plugin-page view models.</summary>
public interface IPluginActionRunner
{
    PluginRunResult PreviewPluginAction(
        string plugin,
        string action,
        Dictionary<string, string> form);

    PluginRunResult RunPluginAction(
        string plugin,
        string action,
        Dictionary<string, string> form);
}
