using System;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Localref.Desktop.Services;

namespace Localref.Desktop.ViewModels;

public partial class RulesWindowViewModel : ViewModelBase
{
    private readonly DaemonService? _daemon;
    private string _savedText = string.Empty;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasUnsavedChanges))]
    private string _rulesText = string.Empty;

    [ObservableProperty]
    private string _statusText = "Rules are stored in library/.localref/rules.toml";

    public bool HasUnsavedChanges => !string.Equals(RulesText, _savedText, StringComparison.Ordinal);

    public RulesWindowViewModel(DaemonService daemon)
    {
        _daemon = daemon;
        Load();
    }

    public RulesWindowViewModel()
    {
    }

    [RelayCommand]
    private void Load()
    {
        if (_daemon is null)
        {
            return;
        }

        try
        {
            RulesText = _daemon.Handle.ReadRulesText();
            _savedText = RulesText;
            OnPropertyChanged(nameof(HasUnsavedChanges));
            StatusText = "Rules loaded";
        }
        catch (Exception ex)
        {
            StatusText = $"Could not load rules: {ex.Message}";
        }
    }

    [RelayCommand]
    private void Save()
    {
        if (_daemon is null)
        {
            return;
        }

        try
        {
            _daemon.Handle.WriteRulesText(RulesText);
            _savedText = RulesText;
            OnPropertyChanged(nameof(HasUnsavedChanges));
            StatusText = "Rules validated and saved";
        }
        catch (Exception ex)
        {
            StatusText = $"Rules are invalid: {ex.Message}";
        }
    }
}
