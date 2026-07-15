//! Plugin identity (`plugin.toml`) and declarative UI spec (`ui.toml`).

use serde::Deserialize;

/// Plugin identity, parsed from `plugin.toml`. No presentation here.
#[derive(Clone, Debug, Deserialize)]
pub struct PluginManifest {
    /// Plugin machine-readable name.
    pub name: String,
    /// CLI executable path relative to the plugin directory.
    #[serde(default)]
    pub executable: Option<String>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Plugin version, injected from the plugin's `Cargo.toml` at build time.
    ///
    /// Absent for hand-authored bundles that were not staged by `build.py`.
    #[serde(default)]
    pub version: Option<String>,
    /// Optional UI-spec filename override (defaults to `ui.toml`).
    #[serde(default)]
    pub ui: Option<String>,
    /// Lifecycle events this plugin runs after (`plugin hook <event>`).
    #[serde(default)]
    pub hooks: Vec<HookBinding>,
    /// Scheduled jobs this plugin runs on a cron timer (`plugin cron <id>`).
    #[serde(default)]
    pub cron: Vec<CronJob>,
    /// Per-item `extra` metadata fields this plugin owns. Declaring a field with
    /// `indexed = true` makes its values participate in search.
    #[serde(default)]
    pub extra_fields: Vec<ExtraFieldDecl>,
}

/// One declared plugin `extra` metadata field.
///
/// Written to `metadata.toml` under `[extra.<namespace>]` as `<key> = <value>`.
/// When `indexed` is set, the daemon includes `namespace.key` in the redb
/// secondary index so its values are searchable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ExtraFieldDecl {
    /// Plugin namespace owning the field (its `[extra.<namespace>]` table).
    pub namespace: String,
    /// Field key within the namespace.
    pub key: String,
    /// Whether this field's values are added to the search index.
    #[serde(default)]
    pub indexed: bool,
}

/// One declared hook: the plugin is spawned after this event completes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub struct HookBinding {
    /// Daemon event this hook fires on.
    pub event: HookEvent,
}

/// Daemon lifecycle events a plugin can bind a hook to. Names match the
/// wire token passed as `plugin hook <event>`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// A new item was imported and indexed.
    ItemImported,
    /// A category was created, renamed, merged, or (un)assigned.
    CategoryChanged,
    /// An indexed item was deleted.
    ItemDeleted,
    /// An item's metadata was patched.
    MetadataPatched,
    /// A full library scan finished.
    ScanCompleted,
    /// A file was attached to an existing item.
    ItemFileAdded,
    /// Automatic-classification rules were replaced.
    RulesChanged,
    /// Runtime plugin schedules were added or removed.
    SchedulesChanged,
    /// A daemon pause mode was enabled.
    DaemonPaused,
    /// A daemon pause mode was disabled.
    DaemonResumed,
}

impl HookEvent {
    /// Stable `snake_case` wire name, matching `DaemonEvent::event_name`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ItemImported => "item_imported",
            Self::CategoryChanged => "category_changed",
            Self::ItemDeleted => "item_deleted",
            Self::MetadataPatched => "metadata_patched",
            Self::ScanCompleted => "scan_completed",
            Self::ItemFileAdded => "item_file_added",
            Self::RulesChanged => "rules_changed",
            Self::SchedulesChanged => "schedules_changed",
            Self::DaemonPaused => "daemon_paused",
            Self::DaemonResumed => "daemon_resumed",
        }
    }
}

/// One scheduled job: the plugin is spawned as `plugin cron <id>` on schedule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct CronJob {
    /// Job id passed back to the plugin as `cron <id>`.
    pub id: String,
    /// Cron expression (6 fields: sec min hour day-of-month month day-of-week).
    pub schedule: String,
    /// Optional subprocess timeout in seconds. Unattended cron syncs raise this
    /// above the host default so a full run isn't cut short.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

impl PluginManifest {
    /// Parse identity from `plugin.toml` source text.
    ///
    /// # Errors
    /// Returns an error when the TOML does not match the identity schema.
    pub fn parse(toml_text: &str) -> Result<Self, crate::PluginError> {
        toml::from_str(toml_text)
            .map_err(|e| crate::PluginError::Parse(e.to_string()))
    }
}

/// Declarative UI spec, parsed from `ui.toml`. Rendered natively by the host.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PluginUiSpec {
    /// Buttons / context-menu entries.
    #[serde(default)]
    pub actions: Vec<UiAction>,
    /// Mounted form pages.
    #[serde(default)]
    pub pages: Vec<UiPage>,
}

impl PluginUiSpec {
    /// Parse a UI spec from `ui.toml` source text.
    ///
    /// # Errors
    /// Returns an error when the TOML does not match the UI-spec schema.
    pub fn parse(toml_text: &str) -> Result<Self, crate::PluginError> {
        toml::from_str(toml_text)
            .map_err(|e| crate::PluginError::Parse(e.to_string()))
    }
}

/// A button or context-menu entry that triggers an action with no form.
#[derive(Clone, Debug, Deserialize)]
pub struct UiAction {
    /// Action id passed to `plugin run <id>`.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Where the action appears.
    #[serde(default)]
    pub mount: UiMount,
    /// Which ids are passed to the spawned action.
    #[serde(default)]
    pub target: UiTarget,
    /// Optional per-action subprocess timeout in seconds. Long-running actions
    /// (e.g. a full-library sync) raise this above the host default.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// A declarative form page mounted at a UI slot.
#[derive(Clone, Debug, Deserialize)]
pub struct UiPage {
    /// Page id.
    pub id: String,
    /// Tab / page label.
    pub label: String,
    /// Where the page is mounted.
    #[serde(default)]
    pub mount: UiMount,
    /// URL route segment.
    pub route: String,
    /// Action id spawned on submit.
    #[serde(default)]
    pub action: Option<String>,
    /// Which ids are passed to the action.
    #[serde(default)]
    pub target: UiTarget,
    /// Data the page consumes. The host derives its contextual surface from
    /// these requirements instead of requiring an explicit mount.
    #[serde(default)]
    pub requires: Vec<UiDataRequirement>,
    /// Optional debounced live-preview callback.
    #[serde(default)]
    pub preview: Option<PreviewSpec>,
    /// Form fields rendered natively by the host.
    #[serde(default)]
    pub fields: Vec<UiField>,
    /// Live-updating readouts (Tier-1 bindings).
    #[serde(default)]
    pub display: Vec<UiDisplay>,
    /// Optional submit presentation. When absent, hosts retain the legacy
    /// generic "Run" button and execute [`Self::action`] directly.
    #[serde(default)]
    pub submit: Option<UiSubmit>,
    /// Optional per-action subprocess timeout in seconds, applied when this
    /// page's `action` is spawned. Raises the host default for slow submits.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// A single declarative form field.
#[derive(Clone, Debug, Deserialize)]
pub struct UiField {
    /// Form field name (becomes `--param name=value`).
    pub name: String,
    /// Display label.
    pub label: String,
    /// Control kind the host renders natively.
    pub kind: FieldKind,
    /// Options for `select` / `radio`; ignored for other kinds.
    #[serde(default)]
    pub options: Vec<String>,
    /// Default value.
    #[serde(default)]
    pub default: Option<String>,
    /// Whether the field is required.
    #[serde(default)]
    pub required: bool,
    /// Tier-1 binding: show only when the expression is truthy.
    #[serde(default)]
    pub show_if: Option<String>,
    /// Tier-1 binding: enable only when the expression is truthy.
    #[serde(default)]
    pub enabled_if: Option<String>,
}

/// A live-updating text readout driven by Tier-1 bindings.
#[derive(Clone, Debug, Deserialize)]
pub struct UiDisplay {
    /// Display id (also the Tier-2 target pane name).
    pub id: String,
    /// Template text with `{selection.count}` / `{field.<name>}` tokens.
    ///
    /// Structured displays (`table` and `details`) render columns rather than
    /// a text template, so their `ui.toml` entries intentionally omit this.
    #[serde(default)]
    pub text: String,
    /// Native surface selected by the host. `text` preserves the original
    /// schema behavior for existing plugin bundles.
    #[serde(default)]
    pub kind: DisplayKind,
    /// Optional display heading.
    #[serde(default)]
    pub title: Option<String>,
    /// Empty-state copy for structured displays.
    #[serde(default)]
    pub empty_text: Option<String>,
    /// Stable columns for a table or selected-row details surface.
    #[serde(default)]
    pub columns: Vec<UiDisplayColumn>,
    /// Field populated by a selected table row's `key` value.
    #[serde(default)]
    pub selection_field: Option<String>,
    /// Table display id whose selected row supplies a details surface.
    #[serde(default)]
    pub selection_of: Option<String>,
}

/// Fixed host-rendered display surfaces.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DisplayKind {
    /// The legacy text/readout pane.
    #[default]
    Text,
    /// A selectable, schema-columned row table.
    Table,
    /// Key/value details for a selected table row.
    Details,
    /// A short informational, warning, or error message.
    Status,
}

/// A stable data column supplied by a structured plugin preview.
#[derive(Clone, Debug, Deserialize)]
pub struct UiDisplayColumn {
    /// Property key in each preview row.
    pub key: String,
    /// User-facing column or details label.
    pub label: String,
}

/// Submit-button presentation and optional high-risk action confirmation.
#[derive(Clone, Debug, Deserialize)]
pub struct UiSubmit {
    /// User-facing submit button label.
    pub label: String,
    /// Optional confirmation shown immediately before the action invocation.
    #[serde(default)]
    pub confirm: Option<UiConfirmation>,
    /// Refresh the declared preview when the action succeeds.
    #[serde(default)]
    pub refresh_after_submit: bool,
}

/// Content rendered in a host-owned confirmation dialog.
#[derive(Clone, Debug, Deserialize)]
pub struct UiConfirmation {
    /// Window title.
    pub title: String,
    /// Body template with `{field.<name>}` substitutions.
    pub message: String,
    /// Confirm button label.
    #[serde(default)]
    pub confirm_label: Option<String>,
}

/// Opt-in debounced live-preview callback (Tier-2).
#[derive(Clone, Debug, Deserialize)]
pub struct PreviewSpec {
    /// Action id spawned to compute the preview.
    pub action: String,
    /// Debounce window in milliseconds.
    pub debounce_ms: u64,
    /// Display id (pane) the returned text is dropped into.
    pub into: String,
}

/// Fixed, host-known field control kinds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// Single-line text input.
    Text,
    /// Multi-line text input.
    Textarea,
    /// Numeric input.
    Number,
    /// Boolean checkbox.
    Checkbox,
    /// Dropdown of `options`.
    Select,
    /// Radio group of `options`.
    Radio,
}

/// UI mount slots shared by actions and pages. Action-only variants (`ActionButton`, `ContextMenu`) and page-only variants (`DetailTab`, `MetadataPage`, `SelectionPage`) are not enforced by the type system; the host ignores invalid pairings.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UiMount {
    /// Top-bar action button (actions only).
    #[default]
    ActionButton,
    /// Item context-menu entry (actions only).
    ContextMenu,
    /// Detail-pane tab (pages only).
    DetailTab,
    /// Single-item metadata page (pages only).
    MetadataPage,
    /// Multi-selection page (pages only).
    SelectionPage,
}

/// Host-provided data a plugin page needs. These requirements determine where
/// the page is offered and when it can open.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UiDataRequirement {
    /// Library-wide data; available from the global Plugin tools menu.
    Library,
    /// The checked item ids; available when one or more items are selected.
    Selection,
    /// The active item id.
    ActiveItem,
    /// Metadata for the active item.
    ItemMetadata,
    /// Attached files for the active item.
    ItemFiles,
    /// Categories for the active item.
    ItemCategories,
    /// The item created by an import-completed event.
    ImportedItem,
}

/// Which item ids the host passes to a spawned action.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UiTarget {
    /// Pass the checked item ids via `--selected`.
    Selection,
    /// Pass the active item id via `--active`.
    Active,
    /// Pass no ids.
    #[default]
    None,
}

#[cfg(test)]
mod tests {
    use super::{
        FieldKind, PluginManifest, PluginUiSpec, UiDataRequirement, UiMount,
        UiTarget,
    };

    #[test]
    fn plugin_manifest_is_identity_only() {
        let m = PluginManifest::parse(
            "name = \"bibtexer\"\nexecutable = \"bibtexer\"\ndescription = \"x\"\n",
        )
        .expect("parse identity");
        assert_eq!(m.name, "bibtexer");
        assert_eq!(m.executable.as_deref(), Some("bibtexer"));
        assert_eq!(m.ui.as_deref(), None);
        // Hooks, cron, and extra_fields are opt-in; absence parses to empty,
        // preserving backward compatibility with existing manifests.
        assert!(m.hooks.is_empty());
        assert!(m.cron.is_empty());
        assert!(m.extra_fields.is_empty());
        // Version is injected at build time; a hand-authored manifest omits it.
        assert_eq!(m.version, None);
    }

    #[test]
    fn plugin_manifest_parses_injected_version() {
        let m = PluginManifest::parse(
            "name = \"s3sync\"\nversion = \"1.2.3\"\nexecutable = \"s3sync\"\n",
        )
        .expect("parse version");
        assert_eq!(m.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn plugin_manifest_parses_extra_fields() {
        let m = PluginManifest::parse(
            r#"
name = "bibtexer"
executable = "bibtexer"

[[extra_fields]]
namespace = "bibtexer"
key = "cite_key"
indexed = true

[[extra_fields]]
namespace = "bibtexer"
key = "note"
"#,
        )
        .expect("parse extra_fields");
        assert_eq!(m.extra_fields.len(), 2);
        assert_eq!(m.extra_fields[0].namespace, "bibtexer");
        assert_eq!(m.extra_fields[0].key, "cite_key");
        assert!(m.extra_fields[0].indexed);
        // `indexed` defaults to false when omitted.
        assert!(!m.extra_fields[1].indexed);
    }

    #[test]
    fn plugin_manifest_parses_hooks_and_cron() {
        let m = PluginManifest::parse(
            r#"
name = "archiver"
executable = "archiver"

[[hooks]]
event = "item_imported"

[[hooks]]
event = "item_deleted"

[[hooks]]
event = "item_file_added"

[[cron]]
id = "nightly_sync"
schedule = "0 0 3 * * *"
"#,
        )
        .expect("parse hooks + cron");
        assert_eq!(
            m.hooks,
            vec![
                super::HookBinding { event: super::HookEvent::ItemImported },
                super::HookBinding { event: super::HookEvent::ItemDeleted },
                super::HookBinding { event: super::HookEvent::ItemFileAdded },
            ],
        );
        assert_eq!(m.cron.len(), 1);
        assert_eq!(m.cron[0].id, "nightly_sync");
        assert_eq!(m.cron[0].schedule, "0 0 3 * * *");
        // Wire name stays in lock-step with the daemon's event name.
        assert_eq!(super::HookEvent::ItemImported.as_str(), "item_imported");
    }

    #[test]
    fn ui_spec_parses_actions_pages_fields_and_preview() {
        let ui = PluginUiSpec::parse(SAMPLE_UI).expect("parse ui spec");
        assert_eq!(ui.actions.len(), 1);
        assert_eq!(ui.actions[0].mount, UiMount::ContextMenu);
        assert_eq!(ui.actions[0].target, UiTarget::Selection);
        let page = &ui.pages[0];
        assert_eq!(page.mount, UiMount::DetailTab);
        assert_eq!(page.requires, vec![UiDataRequirement::Selection]);
        assert_eq!(page.action.as_deref(), Some("export_bibtex"));
        assert_eq!(page.fields[0].kind, FieldKind::Select);
        assert_eq!(
            page.fields[0].options,
            vec!["bibtex".to_string(), "ris".to_string()]
        );
        assert_eq!(page.display[0].text, "Exporting {selection.count} items");
        let preview = page.preview.as_ref().expect("preview present");
        assert_eq!(preview.action, "preview_export");
        assert_eq!(preview.debounce_ms, 300);
        assert_eq!(preview.into, "preview_pane");
    }

    #[test]
    fn ui_target_defaults_to_none() {
        let ui = PluginUiSpec::parse(
            "[[actions]]\nid = \"a\"\nlabel = \"A\"\nmount = \"action_button\"\n",
        )
        .expect("parse");
        assert_eq!(ui.actions[0].target, UiTarget::None);
    }

    #[test]
    fn ui_spec_parses_structured_display_and_confirmation() {
        let ui = PluginUiSpec::parse(
            r#"
[[pages]]
id = "history"
label = "History"
route = "history"
action = "rollback"

[pages.submit]
label = "Restore selected version"
refresh_after_submit = true
confirm = { title = "Restore?", message = "Restore {field.sequence}" }

[[pages.display]]
id = "versions"
text = ""
kind = "table"
selection_field = "sequence"
columns = [{ key = "sequence", label = "Version" }]
"#,
        )
        .expect("parse schema v2");
        let page = &ui.pages[0];
        assert_eq!(page.display[0].kind, super::DisplayKind::Table);
        assert_eq!(
            page.display[0].selection_field.as_deref(),
            Some("sequence")
        );
        assert_eq!(page.display[0].columns[0].key, "sequence");
        assert_eq!(
            page.submit.as_ref().map(|submit| submit.refresh_after_submit),
            Some(true)
        );
    }

    #[test]
    fn structured_displays_do_not_require_a_text_template() {
        let ui = PluginUiSpec::parse(
            r#"
[[pages]]
id = "conflicts"
label = "Conflicts"
route = "conflicts"

[[pages.display]]
id = "blocked_files"
kind = "table"
columns = [{ key = "file", label = "File" }]

[[pages.display]]
id = "selected_file"
kind = "details"
selection_of = "blocked_files"
columns = [{ key = "file", label = "File" }]
"#,
        )
        .expect("structured displays without text must parse");

        assert_eq!(ui.pages[0].display.len(), 2);
        assert!(
            ui.pages[0].display.iter().all(|display| display.text.is_empty())
        );
    }

    #[test]
    fn data_requirements_replace_explicit_mounts() {
        let ui = PluginUiSpec::parse(
            r#"
[[actions]]
id = "global"
label = "Global"

[[pages]]
id = "files"
label = "Files"
route = "files"
target = "active"
requires = ["item_files", "item_categories"]
"#,
        )
        .expect("parse inferred surfaces");

        assert_eq!(ui.actions[0].mount, UiMount::ActionButton);
        assert_eq!(
            ui.pages[0].requires,
            vec![
                UiDataRequirement::ItemFiles,
                UiDataRequirement::ItemCategories,
            ],
        );
    }

    const SAMPLE_UI: &str = r#"
[[actions]]
id = "export_ris"
label = "Export RIS"
mount = "context_menu"
target = "selection"

[[pages]]
id = "export"
label = "Export"
mount = "detail_tab"
route = "export"
action = "export_bibtex"
target = "selection"
requires = ["selection"]
preview = { action = "preview_export", debounce_ms = 300, into = "preview_pane" }

[[pages.fields]]
name = "format"
label = "Format"
kind = "select"
options = ["bibtex", "ris"]
default = "bibtex"

[[pages.display]]
id = "count"
text = "Exporting {selection.count} items"
"#;
}
