using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.Linq;
using Avalonia;
using Avalonia.Media;
using Avalonia.Threading;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Localref.Desktop.Services;
using uniffi.localref_ffi;

namespace Localref.Desktop.ViewModels;

/// <summary>
/// Root library workspace. It keeps native UI state separate from the immutable
/// FFI records and exposes focused commands for the list/detail workflow.
/// </summary>
public partial class MainWindowViewModel : ViewModelBase, DaemonEventListener
{
    private const string AllCategoriesFilter = "All categories";
    private const string UncategorizedFilter = "Uncategorized";
    private readonly DaemonService? _daemon;
    private MetadataDocument? _metadataDocument;
    private string? _loadedMetadataItemId;
    private bool _isRefreshing;

    public ObservableCollection<LibraryItemViewModel> Items { get; } = new();
    public ObservableCollection<LibraryItemViewModel> SelectedItems { get; } = new();
    public ObservableCollection<CategorySummary> Categories { get; } = new();
    public ObservableCollection<string> CategoryFilters { get; } = new()
    {
        AllCategoriesFilter,
        UncategorizedFilter,
    };
    public ObservableCollection<string> Logs { get; } = new();
    public ObservableCollection<ItemFileViewModel> Files { get; } = new();
    public ObservableCollection<string> AssignedCategories { get; } = new();
    public ObservableCollection<string> AvailableCategories { get; } = new();

    [ObservableProperty]
    private string _query = "";

    [ObservableProperty]
    private string _categoryFilter = AllCategoriesFilter;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasSelection))]
    private LibraryItemViewModel? _selectedItem;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(MainFileActionText))]
    private ItemFileViewModel? _selectedFile;

    [ObservableProperty]
    private bool _isFileDragOver;

    [ObservableProperty]
    private string? _selectedAssignedCategory;

    [ObservableProperty]
    private string? _selectedAvailableCategory;

    [ObservableProperty]
    private string _newCategory = "";

    [ObservableProperty]
    private string _statusText = "Ready";

    [ObservableProperty]
    private IBrush _statusBrush = ResolveStatusBrush("Brush.Success");

    [ObservableProperty]
    private string _sortColumn = "Title";

    [ObservableProperty]
    private bool _sortAscending = true;

    [ObservableProperty]
    private string _editTitle = "";

    [ObservableProperty]
    private string _detailAuthors = "";

    [ObservableProperty]
    private string _editType = "";

    [ObservableProperty]
    private string _editYear = "";

    [ObservableProperty]
    private string _editVenue = "";

    [ObservableProperty]
    private string _editDoi = "";

    [ObservableProperty]
    private string _editUri = "";

    [ObservableProperty]
    private string _editAbstract = "";

    [ObservableProperty]
    private bool _hasUnsavedMetadata;

    public bool HasSelection => SelectedItem is not null;
    public bool HasItems => Items.Count > 0;
    public string ResultsText => Items.Count == 1 ? "1 reference" : $"{Items.Count} references";
    public int SelectionCount => SelectedItems.Count;
    public string SelectionText => SelectionCount == 1 ? "1 reference selected" : $"{SelectionCount} references selected";
    public IReadOnlyList<string> SelectedItemIds => SelectedItems.Select(item => item.Id).ToArray();
    public string MainFileActionText => SelectedFile?.IsMain == true ? "Clear main file" : "Set as main";
    public DaemonService? DaemonService => _daemon;
    public string WorkspaceName => _daemon?.RepoName ?? "Localref";
    public event Action? SelectionRestoreRequested;
    public event Action<string>? ItemImported;

    public MainWindowViewModel(DaemonService daemon)
    {
        _daemon = daemon;
        _ = daemon.Handle.SubscribeEvents(this);
        Refresh();
    }

    /// <summary>Previewer constructor. Runtime services are intentionally absent.</summary>
    public MainWindowViewModel() { }

    [RelayCommand]
    public void Refresh()
    {
        if (_daemon is null || _isRefreshing)
        {
            return;
        }

        _isRefreshing = true;
        var activeId = SelectedItem?.Id;
        var selectedIds = SelectedItems.Select(item => item.Id).ToHashSet(StringComparer.Ordinal);
        if (selectedIds.Count == 0 && activeId is not null)
        {
            selectedIds.Add(activeId);
        }
        try
        {
            var categories = _daemon.Handle.ListCategories();
            Replace(Categories, categories);
            var categoryFilters = new[] { AllCategoriesFilter, UncategorizedFilter }
                .Concat(categories.Select(category => category.path))
                .Distinct(StringComparer.OrdinalIgnoreCase)
                .ToArray();
            if (!CategoryFilters.SequenceEqual(categoryFilters, StringComparer.OrdinalIgnoreCase))
            {
                Replace(CategoryFilters, categoryFilters);
            }
            if (!CategoryFilters.Contains(CategoryFilter, StringComparer.OrdinalIgnoreCase))
            {
                CategoryFilter = AllCategoriesFilter;
            }

            var documents = string.IsNullOrWhiteSpace(Query)
                ? _daemon.Handle.ListItems().AsEnumerable()
                : _daemon.Handle.Search(Query)
                    .Select(hit => _daemon.Handle.GetItem(hit.id))
                    .OfType<ItemDocument>();

            documents = CategoryFilter switch
            {
                AllCategoriesFilter => documents,
                UncategorizedFilter => documents.Where(item => item.categories.Length == 0),
                _ => documents.Where(item => item.categories.Contains(
                    CategoryFilter,
                    StringComparer.OrdinalIgnoreCase)),
            };

            Replace(Items, Sort(documents).Select(item => new LibraryItemViewModel(item)));
            Replace(Logs, _daemon.Handle.Events().Select(FormatLog));

            var active = Items.FirstOrDefault(item => item.Id == activeId) ?? Items.FirstOrDefault();
            Replace(SelectedItems, Items.Where(item => selectedIds.Contains(item.Id)));
            if (SelectedItems.Count == 0 && active is not null)
            {
                SelectedItems.Add(active);
            }
            SelectedItem = active;
            UpdateSelectionState();
            SelectionRestoreRequested?.Invoke();

            var status = _daemon.Handle.Status();
            StatusText = status.running
                ? $"Library active · {status.queuedTasks} queued"
                : $"Library idle · {Items.Count} indexed";
            OnPropertyChanged(nameof(ResultsText));
            OnPropertyChanged(nameof(HasItems));
        }
        catch (Exception ex)
        {
            StatusText = $"Could not refresh: {ex.Message}";
        }
        finally
        {
            _isRefreshing = false;
        }
    }

    partial void OnQueryChanged(string value) => Refresh();
    partial void OnCategoryFilterChanged(string value) => Refresh();

    partial void OnSelectedItemChanged(LibraryItemViewModel? value)
    {
        LoadInspector(value);
        OpenFolderCommand.NotifyCanExecuteChanged();
        OpenMainFileCommand.NotifyCanExecuteChanged();
        SaveMetadataCommand.NotifyCanExecuteChanged();
        AddSelectedCategoryCommand.NotifyCanExecuteChanged();
        RemoveSelectedCategoryCommand.NotifyCanExecuteChanged();
    }

    public void SetSelectedItems(IEnumerable<LibraryItemViewModel> items)
    {
        Replace(SelectedItems, items.DistinctBy(item => item.Id));
        UpdateSelectionState();
    }

    partial void OnSelectedFileChanged(ItemFileViewModel? value)
    {
        OpenFileCommand.NotifyCanExecuteChanged();
        ToggleMainFileCommand.NotifyCanExecuteChanged();
    }

    partial void OnSelectedAssignedCategoryChanged(string? value) =>
        RemoveSelectedCategoryCommand.NotifyCanExecuteChanged();

    partial void OnSelectedAvailableCategoryChanged(string? value) =>
        AddSelectedCategoryCommand.NotifyCanExecuteChanged();

    partial void OnEditTitleChanged(string value) => MarkMetadataDirty();
    partial void OnEditTypeChanged(string value) => MarkMetadataDirty();
    partial void OnEditYearChanged(string value) => MarkMetadataDirty();
    partial void OnEditVenueChanged(string value) => MarkMetadataDirty();
    partial void OnEditDoiChanged(string value) => MarkMetadataDirty();
    partial void OnEditUriChanged(string value) => MarkMetadataDirty();
    partial void OnEditAbstractChanged(string value) => MarkMetadataDirty();

    [RelayCommand]
    public void SortBy(string? column)
    {
        if (string.IsNullOrWhiteSpace(column))
        {
            return;
        }

        if (SortColumn == column)
        {
            SortAscending = !SortAscending;
        }
        else
        {
            SortColumn = column;
            SortAscending = true;
        }
        Refresh();
    }

    [RelayCommand]
    public void Scan()
    {
        if (_daemon is null) return;
        try
        {
            _daemon.Handle.ScanAll();
            StatusText = "Library scan requested";
        }
        catch (Exception ex)
        {
            StatusText = $"Scan failed: {ex.Message}";
        }
    }

    private bool CanUseSelection() => _daemon is not null && SelectedItem is not null;

    [RelayCommand(CanExecute = nameof(CanUseSelection))]
    public void OpenFolder()
    {
        if (_daemon is null || SelectedItem is null) return;
        try { _ = _daemon.Handle.OpenItemFolder(SelectedItem.Id); }
        catch (Exception ex) { StatusText = $"Could not open folder: {ex.Message}"; }
    }

    [RelayCommand(CanExecute = nameof(CanUseSelection))]
    public void OpenMainFile()
    {
        if (_daemon is null || SelectedItem is null) return;
        try
        {
            if (!string.IsNullOrWhiteSpace(SelectedItem.Document.mainFile))
            {
                _ = _daemon.Handle.OpenItemFile(SelectedItem.Id, SelectedItem.Document.mainFile);
                StatusText = "Opened main file";
            }
            else
            {
                _ = _daemon.Handle.OpenItemFolder(SelectedItem.Id);
                StatusText = "No main file is set; opened the item folder instead";
            }
        }
        catch (Exception ex)
        {
            StatusText = $"Could not open main file: {ex.Message}";
        }
    }

    private bool CanOpenFile() => _daemon is not null && SelectedItem is not null && SelectedFile is not null;

    [RelayCommand(CanExecute = nameof(CanOpenFile))]
    public void OpenFile()
    {
        if (_daemon is null || SelectedItem is null || SelectedFile is null) return;
        try { _ = _daemon.Handle.OpenItemFile(SelectedItem.Id, SelectedFile.Path); }
        catch (Exception ex) { StatusText = $"Could not open file: {ex.Message}"; }
    }

    public void AddFile(string path)
        => AddFiles(new[] { path });

    public void AddFiles(IEnumerable<string> paths)
    {
        if (_daemon is null || SelectedItem is null) return;
        var added = 0;
        foreach (var path in paths.Distinct(StringComparer.OrdinalIgnoreCase))
        {
            try
            {
                _daemon.Handle.AddFileToItem(SelectedItem.Id, path);
                added++;
            }
            catch (Exception ex)
            {
                StatusText = $"Could not add {System.IO.Path.GetFileName(path)}: {ex.Message}";
            }
        }

        if (added > 0)
        {
            LoadFiles();
            StatusText = added == 1 ? "File added to reference" : $"{added} files added to reference";
        }
    }

    private bool CanToggleMainFile() =>
        CanOpenFile() && SelectedFile?.CanBeMain == true && _metadataDocument is not null;

    [RelayCommand(CanExecute = nameof(CanToggleMainFile))]
    public void ToggleMainFile()
    {
        if (_daemon is null || SelectedItem is null || SelectedFile is null || _metadataDocument is null) return;
        try
        {
            var main = SelectedFile.IsMain ? null : SelectedFile.Path;
            var metadata = _metadataDocument.metadata with
            {
                files = _metadataDocument.metadata.files with { main = main },
            };
            _daemon.Handle.PatchMetadata(SelectedItem.Id, _metadataDocument.metadataRevision, metadata);
            StatusText = main is null ? "Main file cleared" : $"{System.IO.Path.GetFileName(main)} set as main file";
            Refresh();
        }
        catch (Exception ex)
        {
            StatusText = $"Could not update main file: {ex.Message}";
            LoadMetadata();
            LoadFiles();
        }
    }

    private bool CanSaveMetadata() => CanUseSelection() && _metadataDocument is not null;

    [RelayCommand(CanExecute = nameof(CanSaveMetadata))]
    public void SaveMetadata()
    {
        if (_daemon is null || SelectedItem is null || _metadataDocument is null) return;
        if (!string.IsNullOrWhiteSpace(EditYear) && !int.TryParse(EditYear, out _))
        {
            StatusText = "Year must be a whole number";
            return;
        }

        try
        {
            var metadata = _metadataDocument.metadata with
            {
                title = EditTitle.Trim(),
                itemType = EditType.Trim(),
                year = int.TryParse(EditYear, out var year) ? year : null,
                venue = NullIfBlank(EditVenue),
                doi = NullIfBlank(EditDoi),
                uri = NullIfBlank(EditUri),
                abstractNote = NullIfBlank(EditAbstract),
            };
            var saved = _daemon.Handle.PatchMetadata(
                SelectedItem.Id,
                _metadataDocument.metadataRevision,
                metadata);
            HasUnsavedMetadata = false;
            StatusText = "Metadata saved";
            var selectedId = saved.id;
            Refresh();
            SelectedItem = Items.FirstOrDefault(item => item.Id == selectedId);
        }
        catch (Exception ex)
        {
            StatusText = $"Could not save metadata: {ex.Message}";
            LoadMetadata();
        }
    }

    private bool CanCreateCategory() => _daemon is not null && !string.IsNullOrWhiteSpace(NewCategory);

    [RelayCommand(CanExecute = nameof(CanCreateCategory))]
    public void CreateCategory()
    {
        if (_daemon is null || string.IsNullOrWhiteSpace(NewCategory)) return;
        var category = NewCategory.Trim();

        try
        {
            if (!Categories.Any(item => item.path.Equals(category, StringComparison.OrdinalIgnoreCase)))
            {
                _daemon.Handle.CreateCategory(category);
            }
            NewCategory = "";
            Replace(Categories, _daemon.Handle.ListCategories());
            UpdateCategoryBuckets();
            SelectedAvailableCategory = AvailableCategories.FirstOrDefault(path =>
                path.Equals(category, StringComparison.OrdinalIgnoreCase));
            StatusText = $"Created category {category}";
        }
        catch (Exception ex)
        {
            StatusText = $"Could not create category: {ex.Message}";
        }
    }

    partial void OnNewCategoryChanged(string value) => CreateCategoryCommand.NotifyCanExecuteChanged();

    private bool CanAddSelectedCategory() =>
        _daemon is not null && SelectedItems.Count > 0 && !string.IsNullOrWhiteSpace(SelectedAvailableCategory);

    [RelayCommand(CanExecute = nameof(CanAddSelectedCategory))]
    public void AddSelectedCategory()
    {
        if (_daemon is null || SelectedAvailableCategory is null || SelectedItems.Count == 0) return;
        try
        {
            var category = SelectedAvailableCategory;
            _daemon.Handle.AddItemsCategory(SelectedItemIds.ToArray(), category);
            StatusText = $"Added {SelectionText} to {category}";
            Refresh();
        }
        catch (Exception ex)
        {
            StatusText = $"Could not add category: {ex.Message}";
        }
    }

    private bool CanRemoveSelectedCategory() =>
        _daemon is not null && SelectedItems.Count > 0 && !string.IsNullOrWhiteSpace(SelectedAssignedCategory);

    [RelayCommand(CanExecute = nameof(CanRemoveSelectedCategory))]
    public void RemoveSelectedCategory()
    {
        if (_daemon is null || SelectedItems.Count == 0 || SelectedAssignedCategory is null) return;
        try
        {
            var category = SelectedAssignedCategory;
            _daemon.Handle.RemoveItemsCategory(SelectedItemIds.ToArray(), category);
            StatusText = $"Removed {SelectionText} from {category}";
            Refresh();
        }
        catch (Exception ex)
        {
            StatusText = $"Could not remove category: {ex.Message}";
        }
    }

    public void OnEvent(DaemonEvent @event) => Dispatcher.UIThread.Post(() =>
    {
        // This callback is registered with the Rust daemon and can be posted
        // during app teardown, after DaemonService.Stop() has disposed the
        // handle. Every FFI call inside Refresh would then throw, so bail if the
        // daemon is no longer running.
        if (_daemon is not { IsRunning: true })
        {
            return;
        }
        // A plugin-pushed status message carries its own text and severity and
        // must not be clobbered by the idle "N indexed" recompute, so set it
        // and stop before Refresh().
        if (@event is DaemonEvent.StatusMessage status)
        {
            StatusText = status.text;
            StatusBrush = ResolveStatusBrush(status.kind switch
            {
                StatusKind.Success => "Brush.Success",
                StatusKind.Error => "Brush.Danger",
                _ => "Brush.Accent",
            });
            return;
        }
        Refresh();
        if (@event is DaemonEvent.ItemImported imported)
        {
            ItemImported?.Invoke(imported.itemId);
        }
    });

    /// <summary>
    /// Resolve a named design-system brush from application resources, falling
    /// back to a neutral gray when resources are unavailable (e.g. at design
    /// time before the app is built).
    /// </summary>
    private static IBrush ResolveStatusBrush(string key)
    {
        if (Application.Current is { } app
            && app.TryGetResource(key, app.ActualThemeVariant, out var resource)
            && resource is IBrush brush)
        {
            return brush;
        }
        return Brushes.Gray;
    }

    private void LoadInspector(LibraryItemViewModel? item)
    {
        UpdateCategoryBuckets();
        LoadMetadata();
        LoadFiles();
    }

    private void LoadMetadata()
    {
        // A background Refresh reassigns SelectedItem to a fresh view model for
        // the same underlying item, which re-enters LoadMetadata. If the user
        // has unsaved edits on that same item, do not clobber the in-progress
        // fields (or the revision the eventual save is optimistic against) —
        // only reload when the selection actually moved to a different item.
        if (HasUnsavedMetadata
            && SelectedItem is not null
            && SelectedItem.Id == _loadedMetadataItemId)
        {
            return;
        }

        _metadataDocument = _daemon is not null && SelectedItem is not null
            ? _daemon.Handle.GetMetadata(SelectedItem.Id)
            : null;
        _loadedMetadataItemId = SelectedItem?.Id;
        var metadata = _metadataDocument?.metadata;
        EditTitle = metadata?.title ?? "";
        DetailAuthors = FormatAuthors(metadata?.creators);
        EditType = metadata?.itemType ?? "";
        EditYear = metadata?.year?.ToString() ?? "";
        EditVenue = metadata?.venue ?? "";
        EditDoi = metadata?.doi ?? "";
        EditUri = metadata?.uri ?? "";
        EditAbstract = metadata?.abstractNote ?? "";
        HasUnsavedMetadata = false;
    }

    private void LoadFiles()
    {
        var files = _daemon is not null && SelectedItem is not null
            ? _daemon.Handle.ItemFiles(SelectedItem.Id)?.files ?? Array.Empty<ItemFileEntry>()
            : Array.Empty<ItemFileEntry>();
        var main = _metadataDocument?.metadata.files.main ?? SelectedItem?.Document.mainFile;
        Replace(Files, files
            .Select(file => new ItemFileViewModel(file, main))
            .OrderByDescending(file => file.IsMain)
            .ThenBy(file => file.Path));
        SelectedFile = Files.FirstOrDefault(file => file.IsMain) ?? Files.FirstOrDefault();
    }

    private void UpdateSelectionState()
    {
        OnPropertyChanged(nameof(SelectionCount));
        OnPropertyChanged(nameof(SelectionText));
        OnPropertyChanged(nameof(SelectedItemIds));
        UpdateCategoryBuckets();
        AddSelectedCategoryCommand.NotifyCanExecuteChanged();
        RemoveSelectedCategoryCommand.NotifyCanExecuteChanged();
    }

    private void UpdateCategoryBuckets()
    {
        var selected = SelectedItems.Count > 0
            ? SelectedItems.ToArray()
            : SelectedItem is null ? Array.Empty<LibraryItemViewModel>() : new[] { SelectedItem };
        var current = selected.Length == 0
            ? new HashSet<string>(StringComparer.OrdinalIgnoreCase)
            : selected[0].Document.categories.ToHashSet(StringComparer.OrdinalIgnoreCase);
        foreach (var item in selected.Skip(1))
        {
            current.IntersectWith(item.Document.categories);
        }

        Replace(AssignedCategories, current.OrderBy(path => path));
        Replace(AvailableCategories, Categories.Select(item => item.path)
            .Where(path => !current.Contains(path))
            .OrderBy(path => path));
        SelectedAssignedCategory = AssignedCategories.FirstOrDefault();
        SelectedAvailableCategory = AvailableCategories.FirstOrDefault();
    }

    private void MarkMetadataDirty()
    {
        if (_metadataDocument is not null)
        {
            HasUnsavedMetadata = true;
        }
    }

    private static string FormatAuthors(IEnumerable<Creator>? creators)
    {
        var authors = creators?
            .Where(creator => creator.role.Contains("author", StringComparison.OrdinalIgnoreCase))
            .Select(creator => !string.IsNullOrWhiteSpace(creator.name)
                ? creator.name.Trim()
                : string.Join(" ", new[] { creator.given, creator.family }
                    .Where(part => !string.IsNullOrWhiteSpace(part))
                    .Select(part => part!.Trim())))
            .Where(name => !string.IsNullOrWhiteSpace(name))
            .Take(16)
            .ToArray() ?? Array.Empty<string>();

        if (authors.Length == 0)
        {
            return "Unknown author";
        }

        var visibleAuthors = string.Join("; ", authors.Take(15));
        return authors.Length > 15 ? $"{visibleAuthors}; …" : visibleAuthors;
    }

    private IEnumerable<ItemDocument> Sort(IEnumerable<ItemDocument> documents)
    {
        Func<ItemDocument, object?> key = SortColumn switch
        {
            "Author" => item => item.authors.FirstOrDefault(),
            "Year" => item => item.year,
            "Type" => item => item.itemType,
            _ => item => item.title,
        };
        return SortAscending ? documents.OrderBy(key) : documents.OrderByDescending(key);
    }

    private static string? NullIfBlank(string value) => string.IsNullOrWhiteSpace(value) ? null : value.Trim();
    private static string FormatLog(LogEntry entry) => $"{entry.ts}  {entry.level,-5} {entry.message}";

    private static void Replace<T>(ObservableCollection<T> target, IEnumerable<T> source)
    {
        target.Clear();
        foreach (var item in source)
        {
            target.Add(item);
        }
    }
}
