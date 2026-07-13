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
        InitializeComponent();
        Title = title;
        TitleText.Text = title;
        MessageText.Text = message;
        ConfirmButton.Content = confirmLabel;
    }

    private void InitializeComponent() => Avalonia.Markup.Xaml.AvaloniaXamlLoader.Load(this);

    private void OnCancelClick(object? sender, RoutedEventArgs e) => Close(false);

    private void OnConfirmClick(object? sender, RoutedEventArgs e) => Close(true);
}
