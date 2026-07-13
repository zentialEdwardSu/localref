using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using Avalonia.Threading;
using Avalonia.VisualTree;
using Localref.Desktop.Services;
using Localref.Desktop.ViewModels;
using uniffi.localref_ffi;

namespace Localref.Desktop.Views;

public partial class MainWindow : Window
{
    private PluginsWindow? _pluginsWindow;
    private SettingsWindow? _settingsWindow;
    private RulesWindow? _rulesWindow;
    private LogsWindow? _logsWindow;
    private MainWindowViewModel? _subscribedViewModel;
    private bool _restoringSelection;
    private readonly ContextMenu _columnContextMenu = new();
    private readonly ContextMenu _rowContextMenu = new();
    private readonly List<PluginPageWindow> _pluginPageWindows = new();

    public MainWindow()
    {
        InitializeComponent();
        FilesDropTarget.AddHandler(DragDrop.DragOverEvent, OnFilesDragOver);
        FilesDropTarget.AddHandler(DragDrop.DragLeaveEvent, OnFilesDragLeave);
        FilesDropTarget.AddHandler(DragDrop.DropEvent, OnFilesDrop);
        LibraryGrid.AddHandler(
            InputElement.PointerPressedEvent,
            OnLibraryGridPointerPressed,
            RoutingStrategies.Tunnel,
            handledEventsToo: true);
        BuildContextMenus();
        RestoreWorkspacePreferences();
        Closing += (_, _) => SaveWorkspacePreferences();
        DataContextChanged += OnDataContextChanged;
    }

    private MainWindowViewModel? ViewModel => DataContext as MainWindowViewModel;

    private void BuildContextMenus()
    {
        _columnContextMenu.Placement = PlacementMode.Pointer;
        _columnContextMenu.Items.Add(new MenuItem
        {
            Header = "Title",
            ToggleType = MenuItemToggleType.CheckBox,
            IsChecked = true,
            IsEnabled = false,
        });
        foreach (var column in new[] { "Author", "Venue", "Year", "Type", "Categories" })
        {
            var item = new MenuItem
            {
                Header = column,
                Tag = column,
                ToggleType = MenuItemToggleType.CheckBox,
                StaysOpenOnClick = true,
            };
            item.Click += OnColumnMenuItemClick;
            _columnContextMenu.Items.Add(item);
        }
    }

    private void OnLibraryGridPointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (!e.GetCurrentPoint(LibraryGrid).Properties.IsRightButtonPressed || e.Source is not Visual source)
        {
            return;
        }

        var header = source as DataGridColumnHeader ?? source.FindAncestorOfType<DataGridColumnHeader>();
        if (header is not null)
        {
            SyncColumnMenu();
            _columnContextMenu.Open(header);
            e.Handled = true;
            return;
        }

        var row = source as DataGridRow ?? source.FindAncestorOfType<DataGridRow>();
        if (row is not null)
        {
            ActivateContextRow(row);
            BuildRowContextMenu();
            _rowContextMenu.Open(row);
            e.Handled = true;
        }
    }

    private void SyncColumnMenu()
    {
        foreach (var item in _columnContextMenu.Items.OfType<MenuItem>())
        {
            if (item.Tag is string column)
            {
                item.IsChecked = LibraryGrid.Columns.FirstOrDefault(candidate =>
                    string.Equals(candidate.Header?.ToString(), column, StringComparison.Ordinal))?.IsVisible == true;
            }
        }
    }

    private void OnColumnMenuItemClick(object? sender, RoutedEventArgs e)
    {
        if (sender is not MenuItem { Tag: string column } item) return;
        SetColumnVisibility(column, item.IsChecked);
    }

    private void SetColumnVisibility(string column, bool visible)
    {
        var target = LibraryGrid.Columns.FirstOrDefault(candidate =>
            string.Equals(candidate.Header?.ToString(), column, StringComparison.Ordinal));
        if (target is not null)
        {
            target.IsVisible = visible;
            SaveWorkspacePreferences();
        }
    }

    private void RestoreWorkspacePreferences()
    {
        try
        {
            var settings = LocalrefFfiMethods.LoadDesktopUiSettings();
            SetColumnVisibilityWithoutSaving("Author", settings.authorVisible);
            SetColumnVisibilityWithoutSaving("Venue", settings.venueVisible);
            SetColumnVisibilityWithoutSaving("Year", settings.yearVisible);
            SetColumnVisibilityWithoutSaving("Type", settings.typeVisible);
            SetColumnVisibilityWithoutSaving("Categories", settings.categoriesVisible);
            WorkspaceGrid.ColumnDefinitions[2].Width = new GridLength(settings.detailWidth);
        }
        catch
        {
            // Defaults from XAML remain usable when preferences cannot be read.
        }
    }

    private void SetColumnVisibilityWithoutSaving(string column, bool visible)
    {
        var target = LibraryGrid.Columns.FirstOrDefault(candidate =>
            string.Equals(candidate.Header?.ToString(), column, StringComparison.Ordinal));
        if (target is not null)
        {
            target.IsVisible = visible;
        }
    }

    private void OnWorkspaceSplitterDragCompleted(object? sender, VectorEventArgs e) =>
        SaveWorkspacePreferences();

    private void SaveWorkspacePreferences()
    {
        try
        {
            LocalrefFfiMethods.SaveDesktopUiSettings(new DesktopUiSettings(
                authorVisible: IsColumnVisible("Author"),
                venueVisible: IsColumnVisible("Venue"),
                yearVisible: IsColumnVisible("Year"),
                typeVisible: IsColumnVisible("Type"),
                categoriesVisible: IsColumnVisible("Categories"),
                detailWidth: (uint)Math.Round(WorkspaceGrid.ColumnDefinitions[2].ActualWidth)));
        }
        catch
        {
            // A preference write must not interrupt library interaction.
        }
    }

    private bool IsColumnVisible(string column) =>
        LibraryGrid.Columns.FirstOrDefault(candidate =>
            string.Equals(candidate.Header?.ToString(), column, StringComparison.Ordinal))?.IsVisible == true;

    private void OnDataContextChanged(object? sender, EventArgs e)
    {
        if (_subscribedViewModel is not null)
        {
            _subscribedViewModel.SelectionRestoreRequested -= RestoreLibrarySelection;
            _subscribedViewModel.ItemImported -= OnItemImported;
        }
        _subscribedViewModel = ViewModel;
        if (_subscribedViewModel is not null)
        {
            _subscribedViewModel.SelectionRestoreRequested += RestoreLibrarySelection;
            _subscribedViewModel.ItemImported += OnItemImported;
            RestoreLibrarySelection();
        }
    }

    private void OnLibrarySelectionChanged(object? sender, SelectionChangedEventArgs e)
    {
        if (!_restoringSelection && ViewModel is { } viewModel)
        {
            viewModel.SetSelectedItems(LibraryGrid.SelectedItems.OfType<LibraryItemViewModel>());
            UpdateSelectionCheckboxes();
        }
    }

    private void OnRowSelectionCheckBoxClick(object? sender, RoutedEventArgs e)
    {
        if (sender is not CheckBox { DataContext: LibraryItemViewModel item } checkBox)
        {
            return;
        }
        if (checkBox.IsChecked == true)
        {
            if (!LibraryGrid.SelectedItems.Contains(item))
            {
                LibraryGrid.SelectedItems.Add(item);
            }
        }
        else
        {
            LibraryGrid.SelectedItems.Remove(item);
        }
        ViewModel?.SetSelectedItems(LibraryGrid.SelectedItems.OfType<LibraryItemViewModel>());
        UpdateSelectionCheckboxes();
        e.Handled = true;
    }

    private void OnSelectAllClick(object? sender, RoutedEventArgs e)
    {
        _restoringSelection = true;
        try
        {
            LibraryGrid.SelectedItems.Clear();
            if (SelectAllCheckBox.IsChecked == true
                && LibraryGrid.ItemsSource is { } source)
            {
                foreach (var item in source.OfType<LibraryItemViewModel>())
                {
                    LibraryGrid.SelectedItems.Add(item);
                }
            }
        }
        finally
        {
            _restoringSelection = false;
        }
        ViewModel?.SetSelectedItems(LibraryGrid.SelectedItems.OfType<LibraryItemViewModel>());
        UpdateSelectionCheckboxes();
        e.Handled = true;
    }

    private void UpdateSelectionCheckboxes()
    {
        // ItemsSource is null until the Items binding is applied, and can be
        // null transiently during a DataContext swap; a selection event firing
        // in that window must not NRE.
        var source = LibraryGrid.ItemsSource?.OfType<LibraryItemViewModel>()
            ?? Enumerable.Empty<LibraryItemViewModel>();
        var rows = source.ToList();
        var selected = LibraryGrid.SelectedItems.OfType<LibraryItemViewModel>().ToHashSet();
        foreach (var item in rows)
        {
            item.IsSelected = selected.Contains(item);
        }
        SelectAllCheckBox.IsChecked = selected.Count == 0
            ? false
            : selected.Count == rows.Count ? true : null;
    }

    private void RestoreLibrarySelection()
    {
        Dispatcher.UIThread.Post(() =>
        {
            if (ViewModel is not { } viewModel) return;
            _restoringSelection = true;
            try
            {
                LibraryGrid.SelectedItems.Clear();
                foreach (var item in viewModel.SelectedItems.Where(item => item != viewModel.SelectedItem))
                {
                    LibraryGrid.SelectedItems.Add(item);
                }
                if (viewModel.SelectedItem is { } active && viewModel.SelectedItems.Contains(active))
                {
                    LibraryGrid.SelectedItems.Add(active);
                }
            }
            finally
            {
                _restoringSelection = false;
                UpdateSelectionCheckboxes();
            }
        });
    }

    private void OnLibraryDoubleTapped(object? sender, TappedEventArgs e)
    {
        if (e.Source is Visual source && source.FindAncestorOfType<DataGridRow>() is not null)
        {
            ViewModel?.OpenMainFileCommand.Execute(null);
        }
    }

    private void OnFileDoubleTapped(object? sender, TappedEventArgs e) =>
        ViewModel?.OpenFileCommand.Execute(null);

    private void OnOpenFolderClick(object? sender, RoutedEventArgs e) =>
        ViewModel?.OpenFolderCommand.Execute(null);

    private void OnOpenMainFileClick(object? sender, RoutedEventArgs e) =>
        ViewModel?.OpenMainFileCommand.Execute(null);

    private void OnRefreshClick(object? sender, RoutedEventArgs e) =>
        ViewModel?.RefreshCommand.Execute(null);

    private void OnMinimizeClick(object? sender, RoutedEventArgs e) =>
        WindowState = WindowState.Minimized;

    private void OnMaximizeClick(object? sender, RoutedEventArgs e) =>
        WindowState = WindowState == WindowState.Maximized
            ? WindowState.Normal
            : WindowState.Maximized;

    private void OnCloseClick(object? sender, RoutedEventArgs e) => Close();

    private void OnManagePluginsClick(object? sender, RoutedEventArgs e)
    {
        if (ViewModel?.DaemonService is not { } daemon)
        {
            return;
        }

        var pluginContext = new PluginsWindowViewModel(daemon);
        if (_pluginsWindow is not null)
        {
            (_pluginsWindow.DataContext as PluginsWindowViewModel)?.Dispose();
            _pluginsWindow.DataContext = pluginContext;
            _pluginsWindow.Activate();
            return;
        }

        _pluginsWindow = new PluginsWindow
        {
            DataContext = pluginContext,
        };
        _pluginsWindow.Closed += (_, _) =>
        {
            pluginContext.Dispose();
            _pluginsWindow = null;
        };
        _pluginsWindow.Show(this);
    }

    private void OnSettingsClick(object? sender, RoutedEventArgs e)
    {
        if (ViewModel?.DaemonService is not { } daemon)
        {
            return;
        }

        if (_settingsWindow is not null)
        {
            _settingsWindow.Activate();
            return;
        }

        _settingsWindow = new SettingsWindow
        {
            DataContext = new SettingsWindowViewModel(daemon),
        };
        _settingsWindow.Closed += (_, _) => _settingsWindow = null;
        _settingsWindow.Show(this);
    }

    private void OnRulesClick(object? sender, RoutedEventArgs e)
    {
        if (ViewModel?.DaemonService is not { } daemon)
        {
            return;
        }

        if (_rulesWindow is not null)
        {
            _rulesWindow.Activate();
            return;
        }

        _rulesWindow = new RulesWindow
        {
            DataContext = new RulesWindowViewModel(daemon),
        };
        _rulesWindow.Closed += (_, _) => _rulesWindow = null;
        _rulesWindow.Show(this);
    }

    private void OnLogsClick(object? sender, RoutedEventArgs e)
    {
        if (ViewModel is not { } viewModel)
        {
            return;
        }

        if (_logsWindow is not null)
        {
            _logsWindow.Activate();
            return;
        }

        // Reuse the main view model: its Logs collection is already refreshed
        // on every daemon event, so the window updates live without opening a
        // second FFI event subscription (there is no unsubscribe path yet).
        viewModel.Refresh();
        _logsWindow = new LogsWindow
        {
            DataContext = viewModel,
        };
        _logsWindow.Closed += (_, _) => _logsWindow = null;
        _logsWindow.Show(this);
    }

    private void OnPluginToolsSubmenuOpened(object? sender, RoutedEventArgs e)
    {
        if (sender is not MenuItem menu)
        {
            return;
        }

        var items = BuildPluginItems(includeContextActions: false).ToArray();
        menu.Items.Clear();
        foreach (var item in items)
        {
            menu.Items.Add(item);
        }
        if (items.Length == 0)
        {
            menu.Items.Add(new MenuItem
            {
                Header = "No plugin tools for this context",
                IsEnabled = false,
            });
        }
    }

    private void ActivateContextRow(DataGridRow row)
    {
        if (row.DataContext is not LibraryItemViewModel item || LibraryGrid.SelectedItems.Contains(item))
        {
            return;
        }

        LibraryGrid.SelectedItems.Clear();
        LibraryGrid.SelectedItems.Add(item);
        LibraryGrid.SelectedItem = item;
        ViewModel?.SetSelectedItems([item]);
        UpdateSelectionCheckboxes();
    }

    private void BuildRowContextMenu()
    {
        _rowContextMenu.Items.Clear();
        var openMain = new MenuItem { Header = "Open main file" };
        openMain.Click += OnOpenMainFileClick;
        var openFolder = new MenuItem { Header = "Open item folder" };
        openFolder.Click += OnOpenFolderClick;
        _rowContextMenu.Items.Add(openMain);
        _rowContextMenu.Items.Add(openFolder);

        var pluginItems = BuildPluginItems(includeContextActions: true).ToArray();
        if (pluginItems.Length > 0)
        {
            var plugins = new MenuItem { Header = "Plugin tools" };
            foreach (var item in pluginItems)
            {
                plugins.Items.Add(item);
            }
            _rowContextMenu.Items.Add(new Separator());
            _rowContextMenu.Items.Add(plugins);
        }

        var refresh = new MenuItem { Header = "Refresh library" };
        refresh.Click += OnRefreshClick;
        _rowContextMenu.Items.Add(new Separator());
        _rowContextMenu.Items.Add(refresh);

        var selectedCount = ViewModel?.SelectedItemIds.Count ?? 0;
        var delete = new MenuItem
        {
            Header = selectedCount > 1
                ? $"Delete {selectedCount} references…"
                : "Delete reference…",
        };
        delete.Click += OnDeleteItemsClick;
        _rowContextMenu.Items.Add(new Separator());
        _rowContextMenu.Items.Add(delete);
    }

    private async void OnDeleteItemsClick(object? sender, RoutedEventArgs e)
    {
        if (ViewModel is not { DaemonService: { } daemon } viewModel)
        {
            return;
        }

        var items = viewModel.SelectedItems.ToArray();
        if (items.Length == 0)
        {
            return;
        }

        var message = items.Length == 1
            ? $"“{items[0].Title}” and all files attached to it will be permanently deleted."
            : $"The {items.Length} selected references and all files attached to them will be permanently deleted.";

        var deleted = 0;
        try
        {
            // Showing the modal is inside the try: this is an async void handler,
            // so an exception from constructing/showing the dialog (e.g. the
            // owner window closing) would otherwise escape unobserved and tear
            // down the app.
            var confirmed = await new DeleteConfirmationWindow(message)
                .ShowDialog<bool>(this);
            if (!confirmed)
            {
                return;
            }

            foreach (var item in items)
            {
                if (daemon.Handle.DeleteItem(item.Id))
                {
                    deleted++;
                }
            }

            viewModel.Refresh();
            viewModel.StatusText = deleted == 1
                ? "1 reference deleted"
                : $"{deleted} references deleted";
        }
        catch (Exception ex)
        {
            viewModel.Refresh();
            viewModel.StatusText = $"Deleted {deleted} of {items.Length} references: {ex.Message}";
        }
    }

    private IEnumerable<MenuItem> BuildPluginItems(bool includeContextActions)
    {
        if (ViewModel?.DaemonService is not { } daemon)
        {
            yield break;
        }

        var selectedIds = ViewModel.SelectedItemIds;
        var activeId = ViewModel.SelectedItem?.Id;
        PluginDescriptor[] plugins;
        try
        {
            plugins = daemon.Handle.ListPlugins().Where(plugin => plugin.enabled).ToArray();
        }
        catch (Exception ex)
        {
            ViewModel.StatusText = $"Could not load plugin tools: {ex.Message}";
            yield break;
        }

        foreach (var plugin in plugins)
        {
            if (plugin.ui is not { } ui)
            {
                continue;
            }

            foreach (var action in ui.actions.Where(action =>
                         includeContextActions
                             ? action.target is UiTarget.Selection or UiTarget.Active
                             : action.target == UiTarget.None))
            {
                if (!TargetAvailable(action.target, selectedIds, activeId))
                {
                    continue;
                }
                yield return CreatePluginActionItem(daemon, plugin, action, selectedIds, activeId);
            }

            foreach (var page in ui.pages)
            {
                var surface = InferPageSurface(page);
                var available = surface switch
                {
                    PluginPageSurface.Global => true,
                    PluginPageSurface.Selection => selectedIds.Count > 0,
                    PluginPageSurface.ActiveItem => activeId is not null,
                    PluginPageSurface.Import => false,
                    _ => false,
                };
                if (!available ||
                    !RequirementsAvailable(page, selectedIds, activeId, isImport: false) ||
                    !TargetAvailable(page.target, selectedIds, activeId))
                {
                    continue;
                }

                var context = surface == PluginPageSurface.Selection
                    ? $"{selectedIds.Count} selected references"
                    : surface == PluginPageSurface.ActiveItem
                        ? "Active reference"
                        : "Library";
                var item = new MenuItem { Header = $"{plugin.name}: {page.label}" };
                item.Click += (_, _) => OpenPluginPage(plugin, page, selectedIds, activeId, context);
                yield return item;
            }
        }
    }

    private MenuItem CreatePluginActionItem(
        DaemonService daemon,
        PluginDescriptor plugin,
        UiAction action,
        IReadOnlyList<string> selectedIds,
        string? activeId)
    {
        var item = new MenuItem { Header = $"{plugin.name}: {action.label}" };
        item.Click += async (_, _) =>
        {
            var actionViewModel = new PluginActionViewModel(
                daemon,
                plugin.name,
                action,
                selectedIds,
                activeId,
                message =>
                {
                    if (ViewModel is { } viewModel)
                    {
                        viewModel.StatusText = message;
                    }
                },
                SavePluginResultAsync);
            await actionViewModel.Run();
        };
        return item;
    }

    private static bool TargetAvailable(
        UiTarget target,
        IReadOnlyList<string> selectedIds,
        string? activeId) => target switch
    {
        UiTarget.Selection => selectedIds.Count > 0,
        UiTarget.Active => activeId is not null,
        _ => true,
    };

    private void OpenPluginPage(
        PluginDescriptor plugin,
        UiPage page,
        IReadOnlyList<string> selectedIds,
        string? activeId,
        string contextSummary)
    {
        if (ViewModel?.DaemonService is not { } daemon)
        {
            return;
        }

        var window = new PluginPageWindow
        {
            DataContext = new PluginPageWindowViewModel(
                daemon,
                plugin,
                page,
                selectedIds.ToArray(),
                activeId,
                contextSummary),
        };
        _pluginPageWindows.Add(window);
        window.Closed += (_, _) => _pluginPageWindows.Remove(window);
        window.Show(this);
    }

    private void OnItemImported(string itemId)
    {
        if (ViewModel?.DaemonService is not { } daemon)
        {
            return;
        }

        try
        {
            foreach (var plugin in daemon.Handle.ListPlugins().Where(plugin => plugin.enabled))
            {
                foreach (var page in plugin.ui?.pages.Where(page =>
                             InferPageSurface(page) == PluginPageSurface.Import ||
                             (page.requires.Length == 0 && page.mount == UiMount.MetadataPage))
                             ?? Array.Empty<UiPage>())
                {
                    if (RequirementsAvailable(page, [itemId], itemId, isImport: true) &&
                        TargetAvailable(page.target, [itemId], itemId))
                    {
                        OpenPluginPage(plugin, page, [itemId], itemId, "Imported reference");
                    }
                }
            }
        }
        catch (Exception ex)
        {
            ViewModel.StatusText = $"Could not open import plugin tools: {ex.Message}";
        }
    }

    private enum PluginPageSurface
    {
        Global,
        Selection,
        ActiveItem,
        Import,
    }

    private static PluginPageSurface InferPageSurface(UiPage page)
    {
        if (page.requires.Contains(UiDataRequirement.ImportedItem))
        {
            return PluginPageSurface.Import;
        }
        if (page.requires.Any(requirement => requirement is
                UiDataRequirement.ActiveItem or
                UiDataRequirement.ItemMetadata or
                UiDataRequirement.ItemFiles or
                UiDataRequirement.ItemCategories))
        {
            return PluginPageSurface.ActiveItem;
        }
        if (page.requires.Contains(UiDataRequirement.Selection))
        {
            return PluginPageSurface.Selection;
        }
        if (page.requires.Contains(UiDataRequirement.Library))
        {
            return PluginPageSurface.Global;
        }

        // Compatibility path for pre-requirements plugin bundles.
        return page.mount switch
        {
            UiMount.SelectionPage => PluginPageSurface.Selection,
            UiMount.DetailTab or UiMount.MetadataPage => PluginPageSurface.ActiveItem,
            _ => PluginPageSurface.Global,
        };
    }

    private static bool RequirementsAvailable(
        UiPage page,
        IReadOnlyList<string> selectedIds,
        string? activeId,
        bool isImport) => page.requires.All(requirement => requirement switch
    {
        UiDataRequirement.Library => true,
        UiDataRequirement.Selection => selectedIds.Count > 0,
        UiDataRequirement.ActiveItem or
        UiDataRequirement.ItemMetadata or
        UiDataRequirement.ItemFiles or
        UiDataRequirement.ItemCategories => activeId is not null,
        UiDataRequirement.ImportedItem => isImport && activeId is not null,
        _ => false,
    });

    private async Task SavePluginResultAsync(string filename, string content)
    {
        var file = await StorageProvider.SaveFilePickerAsync(new FilePickerSaveOptions
        {
            SuggestedFileName = filename,
        });
        if (file is null)
        {
            return;
        }
        await using var stream = await file.OpenWriteAsync();
        await using var writer = new StreamWriter(stream);
        await writer.WriteAsync(content);
    }

    private async void OnAddFileClick(object? sender, RoutedEventArgs e)
    {
        if (ViewModel is not { HasSelection: true } viewModel)
        {
            return;
        }

        var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = "Add a file to this reference",
            AllowMultiple = true,
        });
        var paths = files.Select(file => file.TryGetLocalPath()).OfType<string>().ToArray();
        if (paths.Length > 0)
        {
            viewModel.AddFiles(paths);
        }
    }

    private void OnFilesDragOver(object? sender, DragEventArgs e)
    {
        var hasFiles = e.DataTransfer.TryGetFiles()?.OfType<IStorageFile>().Any() == true;
        e.DragEffects = hasFiles && ViewModel?.HasSelection == true
            ? DragDropEffects.Copy
            : DragDropEffects.None;
        if (ViewModel is { } viewModel)
        {
            viewModel.IsFileDragOver = e.DragEffects == DragDropEffects.Copy;
        }
        e.Handled = true;
    }

    private void OnFilesDragLeave(object? sender, DragEventArgs e)
    {
        if (ViewModel is { } viewModel)
        {
            viewModel.IsFileDragOver = false;
        }
        e.Handled = true;
    }

    private void OnFilesDrop(object? sender, DragEventArgs e)
    {
        if (ViewModel is not { HasSelection: true } viewModel)
        {
            return;
        }

        viewModel.IsFileDragOver = false;
        var paths = e.DataTransfer.TryGetFiles()?
            .OfType<IStorageFile>()
            .Select(file => file.TryGetLocalPath())
            .OfType<string>()
            .ToArray() ?? Array.Empty<string>();
        if (paths.Length > 0)
        {
            viewModel.AddFiles(paths);
            e.DragEffects = DragDropEffects.Copy;
        }
        e.Handled = true;
    }
}
