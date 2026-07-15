using Avalonia.Controls;
using Avalonia.Interactivity;

namespace Localref.Desktop.Views;

/// <summary>Host-owned confirmation surface for schema-declared plugin actions.</summary>
public partial class PluginConfirmationWindow : Window
{
    public PluginConfirmationWindow() : this("Confirm plugin action", "", "Confirm")
    {
    }

    public PluginConfirmationWindow(string title, string message, string confirmLabel)
    {
        DialogTitle = title;
        DialogMessage = message;
        ConfirmLabel = confirmLabel;
        DataContext = this;
        InitializeComponent();
        Title = title;
    }

    public string DialogTitle { get; }
    public string DialogMessage { get; }
    public string ConfirmLabel { get; }

    private void OnCancelClick(object? sender, RoutedEventArgs e) => Close(false);

    private void OnConfirmClick(object? sender, RoutedEventArgs e) => Close(true);
}
