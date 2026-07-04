using Localref.Desktop.Services;

namespace Localref.Desktop.Tests;

public sealed class WindowsStartupServiceTests
{
    [Fact]
    public void BuildCommandQuotesExecutablePath()
    {
        Assert.Equal(
            "\"C:\\Program Files\\Localref\\Localref.Desktop.exe\"",
            WindowsStartupService.BuildCommand(
                "C:\\Program Files\\Localref\\Localref.Desktop.exe",
                startHidden: false));
    }

    [Fact]
    public void BuildCommandAddsSilentArgumentForHiddenStartup()
    {
        Assert.Equal(
            "\"C:\\Localref\\Localref.Desktop.exe\" --silent",
            WindowsStartupService.BuildCommand(
                "C:\\Localref\\Localref.Desktop.exe",
                startHidden: true));
    }

    [Fact]
    public void StartMenuShortcutUsesStableApplicationName()
    {
        Assert.Equal(
            "C:\\Users\\Ada\\Start Menu\\Programs\\Localref.lnk",
            WindowsStartMenuService.BuildShortcutPath(
                "C:\\Users\\Ada\\Start Menu\\Programs"));
    }
}
