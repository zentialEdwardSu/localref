using System;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace Localref.Desktop.Services;

/// <summary>Registers the unpackaged Windows identity used by toast notifications.</summary>
public static partial class WindowsNotificationIdentity
{
    public const string AppId = "com.localref.Localref.Desktop";
    public const string DisplayName = "Localref";

    public static string? RegistrationError { get; private set; }

    public static void Register()
    {
        if (!OperatingSystem.IsWindows())
        {
            return;
        }

        try
        {
            var result = SetCurrentProcessExplicitAppUserModelID(AppId);
            if (result < 0)
            {
                Marshal.ThrowExceptionForHR(result);
            }

            using var key = Registry.CurrentUser.CreateSubKey(
                $@"Software\Classes\AppUserModelId\{AppId}",
                writable: true);
            if (key is null)
            {
                throw new InvalidOperationException("Could not create the Windows notification identity key.");
            }
            key.SetValue("DisplayName", DisplayName, RegistryValueKind.String);
            key.SetValue("IconBackgroundColor", "0", RegistryValueKind.String);

            var iconPath = Path.Combine(AppContext.BaseDirectory, "localref.ico");
            if (File.Exists(iconPath))
            {
                key.SetValue("IconUri", iconPath, RegistryValueKind.String);
            }
            RegistrationError = null;
        }
        catch (Exception ex)
        {
            RegistrationError = ex.Message;
        }
    }

    [LibraryImport("shell32.dll", StringMarshalling = StringMarshalling.Utf16)]
    private static partial int SetCurrentProcessExplicitAppUserModelID(string appId);
}
