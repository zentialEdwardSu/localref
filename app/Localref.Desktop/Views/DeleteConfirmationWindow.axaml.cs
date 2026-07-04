using Avalonia.Controls;
using Avalonia.Interactivity;

namespace Localref.Desktop.Views;

public partial class DeleteConfirmationWindow : Window
{
    public DeleteConfirmationWindow(string message)
    {
        InitializeComponent();
        MessageText.Text = message;
    }

    public DeleteConfirmationWindow() : this("The selected references and their attached files will be deleted.")
    {
    }

    private void OnCancelClick(object? sender, RoutedEventArgs e) => Close(false);

    private void OnDeleteClick(object? sender, RoutedEventArgs e) => Close(true);
}
