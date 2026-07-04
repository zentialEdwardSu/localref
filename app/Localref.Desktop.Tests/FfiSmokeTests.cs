using System;
using System.Collections.Generic;
using System.IO;
using System.Net.Sockets;
using uniffi.localref_ffi;

namespace Localref.Desktop.Tests;

/// <summary>
/// End-to-end smoke test across the C# ↔ Rust boundary: loads localref_ffi.dll,
/// boots the daemon via the generated bindings, exercises the read path, and
/// shuts down. This is the C# side of the Group B/C verification — it proves the
/// P/Invoke marshalling and DllImport resolution work, not just that Rust builds.
/// </summary>
public class FfiSmokeTests
{
    private static string FreePort()
    {
        var listener = new TcpListener(System.Net.IPAddress.Loopback, 0);
        listener.Start();
        var port = ((System.Net.IPEndPoint)listener.LocalEndpoint).Port;
        listener.Stop();
        return port.ToString();
    }

    [Fact]
    public void StartDaemon_BootsAndServesReads()
    {
        var root = Path.Combine(Path.GetTempPath(), "localref-test-" + Guid.NewGuid());
        var plugins = Path.Combine(root, "plugins");
        Directory.CreateDirectory(plugins);

        var restPort = FreePort();
        var config = new DaemonConfig(
            libraryRoot: root,
            restAddr: $"127.0.0.1:{restPort}",
            cscAddr: $"127.0.0.1:{FreePort()}",
            restEndpoint: $"http://127.0.0.1:{restPort}",
            pluginsDir: plugins);

        var handle = LocalrefFfiMethods.StartDaemon(config);
        try
        {
            Assert.Empty(handle.ListItems());
            Assert.Empty(handle.ListPlugins());
            var status = handle.Status();
            Assert.False(status.running);

            var schedule = new ScheduledCall(
                id: "smoke-schedule",
                plugin: "missing-plugin",
                action: "noop",
                @params: new Dictionary<string, string> { ["format"] = "text" },
                schedule: "0 0 3 * * *");
            handle.RegisterSchedule(schedule);
            Assert.Single(handle.ListSchedules());
            Assert.True(handle.RemoveSchedule(schedule.id));
            Assert.Empty(handle.ListSchedules());
        }
        finally
        {
            handle.Shutdown();
            try { Directory.Delete(root, recursive: true); } catch { /* best effort */ }
        }
    }
}
