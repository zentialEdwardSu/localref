using System.Collections.Generic;
using System.Diagnostics;
using Localref.Desktop.Services;
using Localref.Desktop.ViewModels;
using uniffi.localref_ffi;

namespace Localref.Desktop.Tests;

public sealed class PluginPageViewModelTests
{
    private const string PluginUiJson = "application/vnd.localref.plugin-ui+json;v=1";

    [Fact]
    public void SelectingTableRow_BindsSelectionAndUpdatesDetails()
    {
        var page = Page(
            fields: [Field("sequence", required: true)],
            displays:
            [
                Display("history", DisplayKind.Table, selectionField: "sequence",
                    columns: [Column("sequence"), Column("source")]),
                Display("details", DisplayKind.Details, selectionOf: "history",
                    columns: [Column("sequence"), Column("source")]),
            ]);
        var vm = new PluginPageViewModel(new FakePluginActions(), "s3sync", page);
        var history = vm.Displays[0];
        history.SetRows(
        [
            new Dictionary<string, string>
            {
                ["sequence"] = "42",
                ["source"] = "laptop",
            },
        ]);

        history.SelectedRow = history.Rows.Single();

        Assert.Equal("42", vm.Fields.Single().Value);
        Assert.True(vm.CanRun);
        Assert.Collection(
            vm.Displays[1].Details,
            detail => Assert.Equal(("sequence", "42"), (detail.Label, detail.Value)),
            detail => Assert.Equal(("source", "laptop"), (detail.Label, detail.Value)));
    }

    [Fact]
    public async Task StructuredPreview_RejectsUnknownPaneWithRecoverableError()
    {
        var page = Page(
            preview: new PreviewSpec("list", 0, "history"),
            displays: [Display("history", DisplayKind.Table, columns: [Column("sequence")])]);
        var vm = new PluginPageViewModel(
            new FakePluginActions
            {
                PreviewResult = Ok("{\"unknown\":[{\"sequence\":\"42\"}]}", PluginUiJson),
            },
            "s3sync",
            page);

        await Eventually(() => !string.IsNullOrEmpty(vm.Displays.Single().ErrorText));

        Assert.Contains("Unknown display 'unknown'", vm.Displays.Single().ErrorText);
    }

    [Fact]
    public async Task StalePreviewResponse_DoesNotOverwriteNewerRows()
    {
        var actions = new BlockingPreviewActions(
            Ok("{\"history\":[{\"sequence\":\"old\"}]}", PluginUiJson),
            Ok("{\"history\":[{\"sequence\":\"new\"}]}", PluginUiJson));
        var page = Page(
            preview: new PreviewSpec("list", 0, "history"),
            fields: [Field("query")],
            displays: [Display("history", DisplayKind.Table, columns: [Column("sequence")])]);
        var vm = new PluginPageViewModel(actions, "s3sync", page);
        Assert.True(actions.FirstPreviewStarted.Wait(TimeSpan.FromSeconds(3)));

        vm.Fields.Single().Value = "new query";
        await Eventually(() => vm.Displays.Single().Rows.SingleOrDefault()?.Values["sequence"] == "new");
        actions.ReleaseFirstPreview.Set();
        await Eventually(() => actions.PreviewCalls == 2);

        Assert.Equal("new", vm.Displays.Single().Rows.Single().Values["sequence"]);
    }

    [Fact]
    public async Task CancelledConfirmation_DoesNotRunPluginAction()
    {
        var actions = new FakePluginActions();
        var page = Page(
            action: "restore",
            fields: [Field("sequence", required: true, defaultValue: "42")],
            submit: new UiSubmit("Restore", new UiConfirmation("Confirm", "Restore {field.sequence}", "Restore"), false));
        var vm = new PluginPageViewModel(actions, "s3sync", page);
        vm.ConfirmationRequested += (_, _) => Task.FromResult(false);

        await vm.Run();

        Assert.Equal(0, actions.RunCalls);
    }

    [Fact]
    public async Task SuccessfulSubmit_RefreshesStructuredPreview()
    {
        var actions = new FakePluginActions
        {
            PreviewResult = Ok("{\"history\":[{\"sequence\":\"43\"}]}", PluginUiJson),
        };
        var page = Page(
            action: "restore",
            preview: new PreviewSpec("list", 0, "history"),
            fields: [Field("sequence", required: true, defaultValue: "42")],
            displays: [Display("history", DisplayKind.Table, columns: [Column("sequence")])],
            submit: new UiSubmit("Restore", null, true));
        var vm = new PluginPageViewModel(actions, "s3sync", page);
        await Eventually(() => vm.Displays.Single().Rows.Count == 1);
        var previewsBeforeSubmit = actions.PreviewCalls;

        await vm.Run();
        await Eventually(() => actions.PreviewCalls > previewsBeforeSubmit);

        Assert.Equal(1, actions.RunCalls);
        Assert.Equal("43", vm.Displays.Single().Rows.Single().Values["sequence"]);
    }

    [Fact]
    public async Task SelectionPageWithoutSelection_DoesNotCallPlugin()
    {
        var actions = new FakePluginActions();
        var page = Page(
            target: UiTarget.Selection,
            requirements: [UiDataRequirement.Selection],
            preview: new PreviewSpec("preview", 0, "output"),
            displays: [Display("output", DisplayKind.Text)]);

        var vm = new PluginPageViewModel(actions, "bibtexer", page);
        await vm.Run();

        Assert.False(vm.CanRun);
        Assert.Equal(0, actions.PreviewCalls);
        Assert.Equal(0, actions.RunCalls);
        Assert.Contains("Select at least one reference", vm.ResultText);
        Assert.Contains("Select at least one reference", vm.Displays.Single().ErrorText);
    }

    [Fact]
    public async Task SelectionPageWithSelection_ForwardsSelectionToPreview()
    {
        var actions = new FakePluginActions();
        var page = Page(
            target: UiTarget.Selection,
            requirements: [UiDataRequirement.Selection],
            preview: new PreviewSpec("preview_export", 0, "preview_pane"),
            displays: [Display("preview_pane", DisplayKind.Text)]);

        _ = new PluginPageViewModel(
            actions,
            "bibtexer",
            page,
            ["lr:zotero:selected-item"]);
        await Eventually(() => actions.PreviewCalls == 1);

        Assert.Equal("lr:zotero:selected-item", actions.LastPreviewForm?["selected"]);
    }

    [Fact]
    public async Task ConfirmationFailure_IsContainedAndDoesNotRunPlugin()
    {
        var actions = new FakePluginActions();
        var page = Page(
            action: "restore",
            submit: new UiSubmit("Restore", new UiConfirmation("Confirm", "Continue?", "Restore"), false));
        var vm = new PluginPageViewModel(actions, "s3sync", page);
        vm.ConfirmationRequested += (_, _) =>
            Task.FromException<bool>(new NullReferenceException("confirmation failed"));

        await vm.Run();

        Assert.Equal(0, actions.RunCalls);
    }

    private static UiPage Page(
        string? action = "restore",
        UiTarget target = UiTarget.None,
        UiDataRequirement[]? requirements = null,
        PreviewSpec? preview = null,
        UiField[]? fields = null,
        UiDisplay[]? displays = null,
        UiSubmit? submit = null) =>
        new("page", "Page", UiMount.DetailTab, "/page", action, target, requirements ?? [], preview,
            fields ?? [], displays ?? [], submit);

    private static UiField Field(string name, bool required = false, string? defaultValue = null) =>
        new(name, name, FieldKind.Text, [], defaultValue, required, null, null);

    private static UiDisplay Display(
        string id,
        DisplayKind kind,
        string? selectionField = null,
        string? selectionOf = null,
        UiDisplayColumn[]? columns = null) =>
        new(id, "", kind, id, "No rows.", columns ?? [], selectionField, selectionOf);

    private static UiDisplayColumn Column(string key) => new(key, key);

    private static PluginRunResult Ok(string? content = null, string? contentType = null) =>
        new("ok", content, null, contentType, null);

    private static async Task Eventually(Func<bool> condition)
    {
        var stopwatch = Stopwatch.StartNew();
        while (!condition())
        {
            if (stopwatch.Elapsed > TimeSpan.FromSeconds(3))
            {
                throw new TimeoutException("Condition was not satisfied in time.");
            }
            await Task.Delay(10);
        }
    }

    private sealed class FakePluginActions : IPluginActionRunner
    {
        public PluginRunResult PreviewResult { get; init; } = Ok();
        public int PreviewCalls { get; private set; }
        public int RunCalls { get; private set; }
        public Dictionary<string, string>? LastPreviewForm { get; private set; }

        public PluginRunResult PreviewPluginAction(
            string plugin,
            string action,
            Dictionary<string, string> form)
        {
            PreviewCalls++;
            LastPreviewForm = new Dictionary<string, string>(form);
            return PreviewResult;
        }

        public PluginRunResult RunPluginAction(
            string plugin,
            string action,
            Dictionary<string, string> form)
        {
            RunCalls++;
            return Ok();
        }
    }

    private sealed class BlockingPreviewActions : IPluginActionRunner
    {
        private readonly PluginRunResult _first;
        private readonly PluginRunResult _second;
        private int _previewCalls;

        public BlockingPreviewActions(PluginRunResult first, PluginRunResult second)
        {
            _first = first;
            _second = second;
        }

        public ManualResetEventSlim FirstPreviewStarted { get; } = new();
        public ManualResetEventSlim ReleaseFirstPreview { get; } = new();
        public int PreviewCalls => Volatile.Read(ref _previewCalls);

        public PluginRunResult PreviewPluginAction(
            string plugin,
            string action,
            Dictionary<string, string> form)
        {
            var call = Interlocked.Increment(ref _previewCalls);
            if (call == 1)
            {
                FirstPreviewStarted.Set();
                if (!ReleaseFirstPreview.Wait(TimeSpan.FromSeconds(3)))
                {
                    throw new TimeoutException("The first preview was never released.");
                }
                return _first;
            }
            return _second;
        }

        public PluginRunResult RunPluginAction(
            string plugin,
            string action,
            Dictionary<string, string> form) => Ok();
    }
}
