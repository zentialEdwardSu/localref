using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;

namespace Localref.Desktop.Services;

public sealed class WindowsStartMenuService
{
    private static readonly Guid ShellLinkClassId = new("00021401-0000-0000-C000-000000000046");

    public string AddShortcut()
    {
        if (!OperatingSystem.IsWindows())
        {
            throw new PlatformNotSupportedException("Start menu shortcuts are only available on Windows.");
        }

        var executablePath = Environment.ProcessPath;
        if (string.IsNullOrWhiteSpace(executablePath))
        {
            throw new InvalidOperationException("Could not determine the Localref executable path.");
        }

        var programsPath = Environment.GetFolderPath(Environment.SpecialFolder.Programs);
        if (string.IsNullOrWhiteSpace(programsPath))
        {
            throw new InvalidOperationException("Could not locate the current user's Start menu.");
        }

        Directory.CreateDirectory(programsPath);
        var shortcutPath = BuildShortcutPath(programsPath);
        var workingDirectory = Path.GetDirectoryName(executablePath) ?? AppContext.BaseDirectory;
        var iconPath = Path.Combine(workingDirectory, "localref.ico");
        if (!File.Exists(iconPath))
        {
            iconPath = executablePath;
        }

        var shellLinkType = Type.GetTypeFromCLSID(ShellLinkClassId, throwOnError: true)
            ?? throw new InvalidOperationException("Windows Shell Link support is unavailable.");
        var shellLink = (IShellLinkW)(Activator.CreateInstance(shellLinkType)
            ?? throw new InvalidOperationException("Could not create a Windows Shell Link."));
        try
        {
            shellLink.SetPath(executablePath);
            shellLink.SetWorkingDirectory(workingDirectory);
            shellLink.SetDescription("Localref reference library");
            shellLink.SetIconLocation(iconPath, 0);
            ((IPersistFile)shellLink).Save(shortcutPath, true);
        }
        finally
        {
            Marshal.FinalReleaseComObject(shellLink);
        }

        return shortcutPath;
    }

    internal static string BuildShortcutPath(string programsPath) =>
        Path.Combine(programsPath, "Localref.lnk");

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("000214F9-0000-0000-C000-000000000046")]
    private interface IShellLinkW
    {
        void GetPath(IntPtr file, int maximumPath, IntPtr findData, uint flags);
        void GetIdList(out IntPtr itemIdList);
        void SetIdList(IntPtr itemIdList);
        void GetDescription(IntPtr name, int maximumName);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string name);
        void GetWorkingDirectory(IntPtr directory, int maximumPath);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string directory);
        void GetArguments(IntPtr arguments, int maximumArguments);
        void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string arguments);
        void GetHotkey(out short hotkey);
        void SetHotkey(short hotkey);
        void GetShowCommand(out int showCommand);
        void SetShowCommand(int showCommand);
        void GetIconLocation(IntPtr iconPath, int iconPathLength, out int iconIndex);
        void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string iconPath, int iconIndex);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string path, uint reserved);
        void Resolve(IntPtr windowHandle, uint flags);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string path);
    }
}
