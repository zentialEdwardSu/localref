using System;
using System.IO;
using System.Threading.Tasks;
using Avalonia.Controls;
using Avalonia.Markup.Xaml;
using Avalonia.Platform.Storage;
using Avalonia.VisualTree;
using Localref.Desktop.ViewModels;
using uniffi.localref_ffi;

namespace Localref.Desktop.Views;

/// <summary>
/// View for one plugin page. Wires the view model's <c>SaveRequested</c> to the
/// Avalonia <see cref="IStorageProvider"/> save picker — the native save dialog
/// lives here in the UI process, not in Rust.
/// </summary>
public partial class PluginPageView : UserControl
{
    private PluginPageViewModel? _viewModel;

    public PluginPageView()
    {
        InitializeComponent();
        DataContextChanged += OnDataContextChanged;
    }

    private void InitializeComponent() => AvaloniaXamlLoader.Load(this);

    private void OnDataContextChanged(object? sender, EventArgs e)
    {
        if (_viewModel is not null)
        {
            _viewModel.SaveRequested -= SaveAsync;
            _viewModel.ConfirmationRequested -= ConfirmAsync;
        }
        _viewModel = DataContext as PluginPageViewModel;
        if (_viewModel is { } vm)
        {
            vm.SaveRequested += SaveAsync;
            vm.ConfirmationRequested += ConfirmAsync;
        }
    }

    private async Task SaveAsync(string filename, string content)
    {
        var top = TopLevel.GetTopLevel(this);
        if (top is null)
        {
            return;
        }
        var file = await top.StorageProvider.SaveFilePickerAsync(
            new FilePickerSaveOptions { SuggestedFileName = filename });
        if (file is null)
        {
            return; // user cancelled
        }
        await using var stream = await file.OpenWriteAsync();
        await using var writer = new StreamWriter(stream);
        await writer.WriteAsync(content);
    }

    private async Task<bool> ConfirmAsync(UiConfirmation confirmation, string message)
    {
        var top = TopLevel.GetTopLevel(this) as Window;
        if (top is null)
        {
            return false;
        }
        var dialog = new PluginConfirmationWindow(
            confirmation.title,
            message,
            confirmation.confirmLabel ?? "Confirm");
        return await dialog.ShowDialog<bool>(top);
    }
}
