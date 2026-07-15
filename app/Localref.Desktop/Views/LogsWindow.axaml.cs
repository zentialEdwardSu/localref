using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Input.Platform;
using Localref.Desktop.Services;
using Localref.Desktop.ViewModels;

namespace Localref.Desktop.Views;

public partial class LogsWindow : Window
{
    public LogsWindow()
    {
        InitializeComponent();
    }

    private async void OnCopyAllClick(object? sender, RoutedEventArgs e) =>
        await ExceptionService.Current.RunAsync("Copy daemon log", async () =>
        {
            if (DataContext is not MainWindowViewModel viewModel)
            {
                return;
            }
            if (Clipboard is not { } clipboard)
            {
                viewModel.DaemonLogStatusText = "Clipboard is not available.";
                return;
            }
            await clipboard.SetTextAsync(viewModel.DaemonLogText);
            viewModel.DaemonLogStatusText =
                $"Copied {viewModel.DaemonLogText.Length:N0} characters.";
        }, ExceptionSource.UI);
}
