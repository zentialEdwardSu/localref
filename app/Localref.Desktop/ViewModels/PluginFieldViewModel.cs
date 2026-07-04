using CommunityToolkit.Mvvm.ComponentModel;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>
/// One plugin form field, rendered natively per <see cref="Kind"/>. Holds the
/// live <see cref="Value"/> the user edits; string-typed so every control kind
/// (text, checkbox "true"/"false", select option) maps to a `--param` value.
/// </summary>
public partial class PluginFieldViewModel : ViewModelBase
{
    private readonly UiField _field;

    public PluginFieldViewModel(UiField field)
    {
        _field = field;
        _value = field.@default ?? (field.kind == FieldKind.Checkbox ? "false" : "");
    }

    /// <summary>Form field name; becomes the `--param name=value` key.</summary>
    public string Name => _field.name;

    /// <summary>Display label.</summary>
    public string Label => _field.label;

    /// <summary>Control kind the view switches on.</summary>
    public FieldKind Kind => _field.kind;

    /// <summary>Options for select/radio kinds.</summary>
    public System.Collections.Generic.List<string> Options => new(_field.options);

    /// <summary>Whether this control is a checkbox (for the bool binding).</summary>
    public bool IsCheckbox => _field.kind == FieldKind.Checkbox;

    /// <summary>Whether this control is a dropdown.</summary>
    public bool IsSelect => _field.kind == FieldKind.Select;

    /// <summary>Whether this is a plain text/number/textarea input.</summary>
    public bool IsText =>
        _field.kind is FieldKind.Text or FieldKind.Textarea or FieldKind.Number;

    [ObservableProperty]
    private string _value;

    /// <summary>Checkbox-friendly view over <see cref="Value"/>.</summary>
    public bool BoolValue
    {
        get => Value == "true";
        set => Value = value ? "true" : "false";
    }
}
