# Architecture Rethink Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify the runtime entry point, make Zotero connector imports a single direct path (unmatched items land under `Cat/unmatched/`), and optimize plugin invocation (concurrent slots, lean state) — all wire-compatible with existing plugins.

**Architecture:** A single `AppRuntime::bootstrap` opens the daemon once for every runtime mode. The connector-session assembly sink moves from `src/main.rs` into `crates/core`. The user-deferral "pending import" flow is deleted entirely. Plugin fixed-slot rendering runs concurrently with per-slot timeouts, and plugins receive lean state unless their manifest opts into the heavy fields.

**Tech Stack:** Rust (edition 2024), Axum, Tokio, redb, Leptos (SSR + WASM). Source spec: `docs/superpowers/specs/2026-06-13-architecture-rethink-design.md` (commit b784fd8).

**Conventions (from CLAUDE.md):** Clippy `correctness`/`single_call_fn`/`complexity` are deny-by-default. Edition 2024. Run `cargo test -p <crate>` per crate. Each task ends in a commit.

**Recommended task order:** Tasks are ordered so the codebase compiles after each commit. Connector deletion (Tasks 1-4) lands before the unmatched fallback (Task 5), runtime unification (Tasks 6-8) after, then plugins (Tasks 9-11).

---

## File map

| File | Responsibility | Tasks |
|---|---|---|
| `crates/csc/src/daemon_sink.rs` (new) | Owns connector-session assembly + the `ConnectorImportSink` impl (csc already depends on core; avoids a dependency cycle) | 4 |
| `crates/core/src/pending.rs` (delete) | (removed) user-deferral store | 1 |
| `crates/core/src/lib.rs` | Daemon facade: drop deferral methods, add `unmatched` fallback | 1, 5 |
| `crates/core/src/rest.rs` | Drop 3 pending routes + handlers + test | 2 |
| `crates/ui-app/src/{state.rs,model.rs,dto.rs,app.rs}` | Drop `pending_count` + indicator | 4 |
| `src/rest_client.rs` | Drop `list_pending_imports`, `pending_count` | 2 |
| `src/main.rs` | Add `AppRuntime`, prune modes, remove `LoggingImportSink` | 6, 7, 8 |
| `crates/plugin/src/manifest.rs` | Add `needs_items` / `needs_active_detail` flags | 10 |
| `crates/ui-app/src/server.rs` | Concurrent slot rendering + lean state | 9, 11 |
| `crates/plugin-sdk`, `examples/plugins/bibtexer` | Set new manifest flags | 10 |

---

## Task 1: Remove the user-deferral pending-import flow from core

**Files:**
- Delete: `crates/core/src/pending.rs`
- Modify: `crates/core/src/lib.rs` (the `pub mod pending;` declaration ~line 23, the `pub use pending::{...}` re-export ~lines 50-52, the `pending` field ~line 213, its initializer ~line 241, and the four deferral methods ~lines 392-490)

This is a deletion task: no new behavior, so the verification is "the crate still compiles and its remaining tests pass." The unmatched-import behavior that replaces deferral is Task 5.

- [ ] **Step 1: Delete the module file**

```bash
git rm crates/core/src/pending.rs
```

- [ ] **Step 2: Remove the module declaration and re-export in `lib.rs`**

Delete the line:
```rust
pub mod pending;
```
And delete the re-export block:
```rust
pub use pending::{
    PendingImportConfirmation, PendingImportSession, PendingImportStore,
};
```

- [ ] **Step 3: Remove the `pending` field from the daemon struct**

In the struct that holds daemon state (around line 213), delete:
```rust
    /// Stored pending.
    pending: PendingImportStore,
```
And in its constructor (around line 241), delete:
```rust
            pending: PendingImportStore::default(),
```

- [ ] **Step 4: Delete the four deferral methods**

Delete `create_pending_connector_import`, `pending_imports`, `confirm_pending_import`, and `cancel_pending_import` (the contiguous block ~lines 392-490). Also delete the now-unused `LogKind::ImportPendingUserConfirmation` variant — search for `ImportPendingUserConfirmation` and remove its enum variant and its `as_str` arm.

- [ ] **Step 5: Build to find all references**

Run: `cargo build -p localref-core`
Expected: compile errors ONLY from `rest.rs` referencing the deleted methods (fixed in Task 2). If errors appear elsewhere in core, remove those references too. Re-run until the only remaining errors are in `rest.rs`.

- [ ] **Step 6: Commit (deferred until Task 2 — core + rest must compile together)**

Do not commit yet; `rest.rs` still references the deleted methods. Proceed directly to Task 2, then commit both together.

---

## Task 2: Remove pending routes from REST and the REST client

**Files:**
- Modify: `crates/core/src/rest.rs` (routes ~lines 101-106, handlers `pending_imports`/`confirm_pending_import`/`cancel_pending_import` ~lines 511-540, the import on line 15, the `confirms_pending_imports` test ~lines 647-679)
- Modify: `src/rest_client.rs` (`DashboardSnapshot.pending_count` line 27, `dashboard_snapshot` line 128, `list_pending_imports` ~lines 253-258, and any `PendingImportSummary` type)

- [ ] **Step 1: Remove the three routes**

In `rest.rs`, delete these route registrations (~lines 101-106):
```rust
        .route("/api/import/pending", get(pending_imports))
        .route(
            "/api/import/pending/{id}/confirm",
            post(confirm_pending_import),
        )
        .route("/api/import/pending/{id}/cancel", post(cancel_pending_import))
```

- [ ] **Step 2: Remove the handlers and the import**

Delete the `pending_imports`, `confirm_pending_import`, and `cancel_pending_import` handler functions (~lines 511-540). On line 15, change:
```rust
use crate::{LocalrefDaemon, PauseMode, PendingImportConfirmation};
```
to:
```rust
use crate::{LocalrefDaemon, PauseMode};
```

- [ ] **Step 3: Delete the `confirms_pending_imports` test**

Delete the entire `confirms_pending_imports` test function (~lines 647-679).

- [ ] **Step 4: Fix the REST client**

In `src/rest_client.rs`: delete the `pending_count` field from `DashboardSnapshot` (line 27), delete the `pending_count:` line from `dashboard_snapshot` (line 128), delete the `list_pending_imports` method (~lines 253-258), and delete the `PendingImportSummary` struct/import (search for `PendingImportSummary` and remove its definition and any `use`).

- [ ] **Step 4b: Fix the tray snapshot printer in `src/main.rs`**

`run_tray_action` prints `snapshot.pending_count` (line 312). Remove it. Change the block (~lines 308-314):
```rust
            println!(
                "Localref: items={} categories={} pending={} logs={}",
                snapshot.item_count,
                snapshot.category_count,
                snapshot.pending_count,
                snapshot.log_count
            );
```
to:
```rust
            println!(
                "Localref: items={} categories={} logs={}",
                snapshot.item_count,
                snapshot.category_count,
                snapshot.log_count
            );
```

- [ ] **Step 5: Build the workspace**

Run: `cargo build -p localref-core && cargo build -p localref`
Expected: PASS (no references to deleted symbols remain).

- [ ] **Step 6: Test and commit**

Run: `cargo test -p localref-core`
Expected: PASS (all remaining tests).

```bash
git add crates/core/src/lib.rs crates/core/src/rest.rs src/rest_client.rs src/main.rs
git commit -m "refactor: remove user-deferral pending-import flow"
```

---

## Task 3: Remove the `pending_count` UI indicator

**Files:**
- Modify: `crates/ui-app/src/state.rs` (`pending_count` field line 58, assignment line 118 `let pending_count = daemon.pending_imports().len();`, struct init line 164)
- Modify: `crates/ui-app/src/model.rs` (`pending_count` field ~lines 20-21)
- Modify: `crates/ui-app/src/dto.rs` (`pending_count: model.pending_count` ~line 34)
- Modify: `crates/ui-app/src/app.rs` (the "{n} pending" view ~line 164)

This is a deletion task — verification is "ui-app compiles." The `daemon.pending_imports()` call no longer exists (deleted in Task 1), so this task is also required for the workspace to build with the UI feature.

- [ ] **Step 1: Remove from `state.rs`**

Delete the field (lines 57-58):
```rust
    /// Stored pending count.
    pub(crate) pending_count: usize,
```
Delete the binding (line 118):
```rust
        let pending_count = daemon.pending_imports().len();
```
Delete `pending_count,` from the struct initializer (line 164).

- [ ] **Step 2: Remove from `model.rs`**

Delete (lines 20-21):
```rust
    /// Number of pending imports.
    pub pending_count: usize,
```

- [ ] **Step 3: Remove from `dto.rs`**

Delete (line 34):
```rust
        pending_count: model.pending_count,
```

- [ ] **Step 4: Remove the indicator from `app.rs`**

Delete the view fragment (line 164):
```rust
                    <span>{move || state.with(|state| state.pending_count)} " pending"</span>
```

- [ ] **Step 5: Build ui-app (both feature sets)**

Run: `cargo build -p ui-app`
Expected: PASS.
Run: `cargo build -p ui-app --target wasm32-unknown-unknown --no-default-features --features hydrate`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/ui-app/src/state.rs crates/ui-app/src/model.rs crates/ui-app/src/dto.rs crates/ui-app/src/app.rs
git commit -m "refactor: remove pending-import UI indicator"
```

---

## Task 4: Relocate the connector-session sink out of the binary

**Files:**
- Create: `crates/csc/src/daemon_sink.rs`
- Modify: `crates/csc/src/lib.rs` (add `mod daemon_sink; pub use daemon_sink::DaemonConnectorSink;`)

**⚠ Dependency-direction constraint (verified):** `csc` already depends on `localref-core` (`crates/csc/Cargo.toml:15`), and `csc` defines the `ConnectorImportSink` trait (`crates/csc/src/lib.rs:211`) precisely to "keep `csc` independent from `core`". Therefore the sink **cannot** live in `core` and implement the csc trait there — that would require `core → csc`, creating a `core → csc → core` cycle that Cargo rejects.

**Resolution (deviation from spec §2 wording):** The spec said "move the sink into core." That is not buildable given the existing dependency graph. The sink instead moves into the **`csc` crate** (which already depends on core and owns the trait). This still achieves the spec's actual goal — *removing the import logic from `src/main.rs` so the binary is thin* — and core still owns the *import pipeline* (`import_connector_item`); only the protocol-assembly adapter sits in csc, next to the trait it implements. The binary keeps only "wire the csc server to a `DaemonConnectorSink`." Flag this deviation when reporting completion.

- [ ] **Step 1: Create `daemon_sink.rs` with the relocated sink**

Create `crates/csc/src/daemon_sink.rs` containing the `LoggingImportSink` logic from `src/main.rs:353-577`, renamed `DaemonConnectorSink`, with these changes:
- Hold a `localref_core::LocalrefDaemon` (csc already imports `localref_core`).
- Implement the local `crate::ConnectorImportSink` trait (same crate).
- Use `localref_core::types::{ConnectorAttachment, ConnectorImport, ConnectorItem, ImportOutcome}` and `crate::{ConnectorEvent, ConnectorImportRequest}`.
- Drop every `println!(...)` line (console noise belongs to the CLI, not the library). Keep all `tracing::info!` calls.
- Rename the internal `PendingImport` buffer struct to `ConnectorSession` (avoid confusion with the deleted deferral flow).
- Keep `try_import_locked`, `accept_import`, `accept_attachment`, `accept_event`, `category_paths`, and the free fn `standalone_attachment_import` (now private to this module) exactly as in main.rs, minus println.

Public constructor:
```rust
impl DaemonConnectorSink {
    pub fn new(daemon: localref_core::LocalrefDaemon) -> Self {
        Self { daemon, sessions: std::sync::Mutex::new(Vec::new()) }
    }
}
```

- [ ] **Step 2: Declare and export the module**

In `crates/csc/src/lib.rs`, add near the top:
```rust
mod daemon_sink;
pub use daemon_sink::DaemonConnectorSink;
```

- [ ] **Step 3: Build csc**

Run: `cargo build -p csc`
Expected: PASS. (main.rs still has its own copy; the duplicate is removed in Task 8.)

- [ ] **Step 4: Commit**

```bash
git add crates/csc/src/daemon_sink.rs crates/csc/src/lib.rs
git commit -m "refactor: move connector-session sink into csc crate"
```

---

## Task 5: Land unmatched connector imports under `Cat/unmatched/`

**Files:**
- Modify: `crates/core/src/lib.rs` (`import_connector_item_with_categories` ~lines 1505-1595; add `UNMATCHED_CATEGORY` const)
- Test: add to the existing `#[cfg(test)] mod tests` in `crates/core/src/rest.rs` (line 552) — this is where core's connector-import tests live (`category_write_endpoints_update_cat_links` at line 937 is the template). It already imports `ConnectorImport, ConnectorItem` (line 554), `CategoryPath` and `LocalrefDaemon` via `use super::*`, and `json!`.

**Context:** Today an import with no matching rule gets an empty `categories` vec and is linked nowhere in `Cat/`. New behavior: substitute the `unmatched` category so every item is always discoverable. This is the spec's core new behavior — it gets a direct test (spec Section 5).

- [ ] **Step 1: Write the failing test**

Add this test inside `mod tests` in `crates/core/src/rest.rs`, following the `category_write_endpoints_update_cat_links` pattern (line 937). It imports an item whose metadata matches no rule and asserts it lands in `All/` and is linked under `Cat/unmatched/`:

```rust
    #[test]
    fn unmatched_connector_import_links_under_unmatched_category() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let outcome = daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("s-unmatched".to_string()),
                    uri: None,
                    connector_item_id: Some("unmatched-1".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Totally Unclassifiable Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Totally Unclassifiable Paper"}),
                },
                attachments: Vec::new(),
            })
            .expect("import should succeed");

        // Item exists in All/
        assert!(outcome.item_dir.exists(), "item dir written to All/");
        // Classified as the unmatched category
        assert_eq!(
            outcome.categories,
            vec![CategoryPath::new("unmatched").unwrap()],
            "unmatched import must be classified as 'unmatched'",
        );
        // Linked under Cat/unmatched/
        assert!(
            temp.path().join("Cat").join("unmatched").exists(),
            "Cat/unmatched/ must exist after an unmatched import",
        );
    }
```

Note: this is a plain `#[test]` (not `#[tokio::test]`) — `import_connector_item` is synchronous.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p localref-core unmatched_connector_import_links_under_unmatched_category`
Expected: FAIL — `outcome.categories` is empty `[]`, not `["unmatched"]`, and `Cat/unmatched/` does not exist.

- [ ] **Step 3: Add the constant and the fallback**

Near the top of `lib.rs` (with other consts), add:
```rust
/// Category assigned to connector imports that match no classification rule.
const UNMATCHED_CATEGORY: &str = "unmatched";
```

In `import_connector_item_with_categories`, immediately after `let import = import.borrow();` and the title check, normalize the categories. Change the signature body so categories defaults to unmatched when empty. The cleanest spot: right before the `for category in &categories` loop (~line 1573), replace the incoming `categories` binding:

```rust
        let categories = if categories.is_empty() {
            vec![CategoryPath::new(UNMATCHED_CATEGORY)
                .expect("UNMATCHED_CATEGORY is a valid category path")]
        } else {
            categories
        };
```

Then delete the now-redundant `if !categories.is_empty()` guard around the `AutoClassifiedOnImport` log (~line 1584) — categories is always non-empty now, so unwrap the log to run unconditionally.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p localref-core unmatched_connector_import_links_under_unmatched_category`
Expected: PASS.

- [ ] **Step 5: Run the full core test suite**

Run: `cargo test -p localref-core`
Expected: PASS — existing connector-import-with-rules tests still green (matched imports unaffected).

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/lib.rs
git commit -m "feat: link unmatched connector imports under Cat/unmatched/"
```

---

## Task 6: Introduce `AppRuntime` with a single `bootstrap`

**Files:**
- Modify: `src/main.rs` (add the `AppRuntime` struct + `bootstrap`; refactor `open_daemon` callers)

**Context:** Today the daemon is opened three ways: `open_daemon` (line 146), a hand-built `StorageDb`+`LocalrefDaemon` in `serve_rest` (lines 193-195), and `LocalrefDaemon::for_library` in `serve_csc_only` (line 241). `AppRuntime::bootstrap` becomes the one path. This task adds the struct and routes the *surviving* code through it; mode pruning is Task 7.

Note `LocalrefDaemon` is `Clone` (it is cloned at lines 128/140/171). `AppRuntime` holds it directly and hands out clones.

- [ ] **Step 1: Add the `AppRuntime` struct and `bootstrap`**

Add near the top of `src/main.rs` (after the imports, before `fn main`):

```rust
/// Process-wide runtime built once and shared by every mode.
struct AppRuntime {
    config: LocalrefConfig,
    daemon: LocalrefDaemon,
    plugins: Arc<Vec<localref_plugin::DiscoveredPlugin>>,
}

impl AppRuntime {
    /// Open the daemon and discover plugins once.
    fn bootstrap(config: LocalrefConfig) -> std::io::Result<Self> {
        let storage = StorageDb::open(config.library_root())
            .map_err(std::io::Error::other)?;
        let daemon = LocalrefDaemon::new(storage);
        let plugins =
            Arc::new(localref_plugin::discover_plugins(config.plugins_dir()));
        Ok(Self { config, daemon, plugins })
    }
}
```

Verify the `DiscoveredPlugin` path: run `grep -n "pub use\|pub struct DiscoveredPlugin\|DiscoveredPlugin" crates/plugin/src/lib.rs` and use whatever path is re-exported (likely `localref_plugin::DiscoveredPlugin` or `localref_plugin::discovery::DiscoveredPlugin`). Match the path already used in `rest_app` (line 217-218 uses `localref_plugin::discover_plugins`).

- [ ] **Step 2: Build to confirm the struct compiles**

Run: `cargo build -p localref`
Expected: PASS, with a `dead_code` warning for `AppRuntime` (not yet used). The warning is expected — it is consumed in Task 7.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: add AppRuntime with single bootstrap path"
```

---

## Task 7: Prune runtime modes to `tray-host`, `ui`, `tray`

**Files:**
- Modify: `src/main.rs` (`AppCommand` enum lines 63-83, `main` dispatch lines 37-50, delete `serve_all`/`serve_rest`/`serve_csc_only`)

**Context:** Spec Section 1 removes `headless`/`serve`, `rest`, and the in-`main` `csc` mode. The standalone `src/bin/localref-csc-dev.rs` binary is untouched. After this task, `AppRuntime` from Task 6 is wired into the surviving modes (`tray-host` and `ui`), clearing its dead-code warning.

- [ ] **Step 1: Reduce the `AppCommand` enum**

Replace the enum (lines 63-83) with:
```rust
/// Runtime command selected from CLI arguments.
#[derive(Clone, Debug, Default, Eq, PartialEq, Subcommand)]
enum AppCommand {
    /// Start the tray-resident daemon process.
    #[default]
    TrayHost,
    /// Open the browser-served UI.
    Ui,
    /// Execute one tray action through the same binary.
    Tray {
        /// Tray action to execute. Defaults to refreshing status.
        #[command(subcommand)]
        action: Option<TrayCliAction>,
    },
}
```
(`#[derive(Default)]` + `#[default]` replaces the `.unwrap_or(AppCommand::TrayHost)` in `main` — but if `clap` rejects `Default` on a subcommand enum, keep the explicit `.unwrap_or(AppCommand::TrayHost)` and drop the derive. Verify in Step 3.)

- [ ] **Step 2: Update the `main` dispatch**

Replace the match (lines 37-50) with:
```rust
    match cli.command.unwrap_or(AppCommand::TrayHost) {
        AppCommand::TrayHost => {
            let runtime = AppRuntime::bootstrap(config)?;
            run_tray_host(runtime)
        }
        AppCommand::Ui => launch_ui().map_err(std::io::Error::other),
        AppCommand::Tray { action } => {
            run_tray_action(
                &config,
                action.map(Into::into).unwrap_or(TrayAction::RefreshStatus),
            );
            Ok(())
        }
    }
```
Note: `config` is moved into `bootstrap` on the `TrayHost` arm but borrowed on the `Tray` arm. Since the match consumes `cli.command` not `config`, and only one arm runs, this is fine — but the `Ui` arm uses neither (it reloads config itself in `launch_ui`). If the borrow checker complains about `config` partial move, restructure so `Tray` is matched first (borrow) — but a `match` on a moved-out value only runs one arm, so no conflict arises.

- [ ] **Step 3: Rewrite `run_tray_host` to take `AppRuntime`**

Replace `run_tray_host` (lines 119-133) with:
```rust
/// Start the tray-hosted daemon runtime.
fn run_tray_host(runtime: AppRuntime) -> std::io::Result<()> {
    tracing::info!(target: "localref::tray_host", "tray host starting");
    if runtime.config.desktop_quiet_start() {
        detach_console_for_quiet_start();
    } else {
        print_config_summary(&runtime.config);
    }
    let print_listeners = !runtime.config.desktop_quiet_start();
    let _api_thread = start_api_runtime(runtime, print_listeners)?;
    // run_native_tray_host needs the config; clone it before moving runtime.
    // (Handled by passing config into start_api_runtime and keeping a clone.)
    Ok(())
}
```
This needs `run_native_tray_host(&config)` to still run after starting the API. Restructure: clone `runtime.config` before consuming `runtime`:
```rust
fn run_tray_host(runtime: AppRuntime) -> std::io::Result<()> {
    tracing::info!(target: "localref::tray_host", "tray host starting");
    let config = runtime.config.clone();
    if config.desktop_quiet_start() {
        detach_console_for_quiet_start();
    } else {
        print_config_summary(&config);
    }
    let print_listeners = !config.desktop_quiet_start();
    let _api_thread = start_api_runtime(runtime, print_listeners)?;
    run_native_tray_host(&config)
}
```

- [ ] **Step 4: Update `start_api_runtime` to take `AppRuntime`**

Replace `start_api_runtime` (lines 163-189) so it consumes the runtime and builds both servers from it:
```rust
/// Start REST and CSC servers on a background Tokio runtime.
fn start_api_runtime(
    runtime: AppRuntime,
    _print_listeners: bool,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("localref-api-runtime".to_string())
        .spawn(move || {
            let tokio_rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to start Localref API runtime");
            tokio_rt.block_on(async move {
                let rest = serve_rest_with_runtime(&runtime);
                let csc = serve_csc_with_runtime(&runtime);
                if let Err(error) = tokio::try_join!(rest, csc).map(|_| ()) {
                    tracing::error!(
                        target: "localref::runtime",
                        "localref API runtime stopped: {error}",
                    );
                    eprintln!("localref API runtime stopped: {error}");
                }
            });
        })
}
```

- [ ] **Step 5: Delete the dead mode functions**

Delete `serve_all` (lines 135-143), `serve_rest` (lines 191-197), `serve_csc_only` (lines 239-244), and `run_runtime` (lines 152-160, no longer called). Delete `open_daemon` (lines 145-150, superseded by `AppRuntime::bootstrap`). Keep `serve_rest_with_daemon`, `serve_csc_with_daemon`, and `rest_app` for now — Task 8 adapts them into `serve_rest_with_runtime` / `serve_csc_with_runtime`.

- [ ] **Step 6: Add thin runtime-based server wrappers**

Add:
```rust
/// Start the REST + UI listener from the shared runtime.
async fn serve_rest_with_runtime(runtime: &AppRuntime) -> std::io::Result<()> {
    serve_rest_with_daemon(runtime.config.clone(), runtime.daemon.clone()).await
}

/// Start the connector listener from the shared runtime.
async fn serve_csc_with_runtime(runtime: &AppRuntime) -> std::io::Result<()> {
    serve_csc_with_daemon(runtime.config.clone(), runtime.daemon.clone()).await
}
```
(Task 8 replaces the sink inside `serve_csc_with_daemon`. `rest_app` will be adjusted in Task 8 to reuse `runtime.plugins` instead of re-discovering — for now it still re-discovers, which is correct behavior, just duplicated.)

- [ ] **Step 7: Build and check CLI help**

Run: `cargo build -p localref`
Expected: PASS, no `dead_code` warning for `AppRuntime`.
Run: `cargo run -p localref -- --help`
Expected: only `tray-host`, `ui`, `tray` subcommands listed; no `headless`/`rest`/`csc`.

- [ ] **Step 8: Commit**

```bash
git add src/main.rs
git commit -m "refactor: prune runtime modes to tray-host/ui/tray"
```

---

## Task 8: Use the core-owned connector sink; delete the duplicate

**Files:**
- Modify: `src/main.rs` (delete `LoggingImportSink` + `PendingImport` + `standalone_attachment_import` lines 353-577; adjust `serve_csc_with_daemon`; thread `runtime.plugins` into `rest_app`)

**Context:** Task 4 created `csc::DaemonConnectorSink`. Now the binary uses it and deletes its own copy. Also wire `AppRuntime.plugins` into `rest_app` so plugins are discovered once (in `bootstrap`) instead of per-listener.

- [ ] **Step 1: Point the CSC server at the core sink**

In `serve_csc_with_daemon` (line 247), replace:
```rust
    let sink = Arc::new(LoggingImportSink::new(daemon));
```
with:
```rust
    let sink = Arc::new(csc::DaemonConnectorSink::new(daemon));
```

- [ ] **Step 2: Delete the binary's sink code**

Delete `LoggingImportSink` (struct lines 354-357), `PendingImport` (lines 360-366), the `impl LoggingImportSink` block (368-410), the `impl ConnectorImportSink for LoggingImportSink` block (412-546), and `standalone_attachment_import` (549-577). Remove now-unused imports at the top: `ConnectorEvent, ConnectorImportRequest, ConnectorImportSink` (line 16-19) and `ConnectorAttachment, ConnectorImport, ConnectorItem, ImportOutcome` (line 23-25) — keep only what the file still uses (run the build to see what's unused). Keep `use csc::serve as serve_csc;`.

- [ ] **Step 3: Thread `runtime.plugins` into `rest_app`**

Change `rest_app` to accept pre-discovered plugins. Replace its signature/body (lines 216-231) so it takes `&AppRuntime` (or `plugins: Arc<...>` + config + daemon) and uses `runtime.plugins.clone()` instead of calling `discover_plugins` again. Update `serve_rest_with_runtime` to call the new form. Concretely:
```rust
#[cfg(feature = "desktop")]
fn rest_app(runtime: &AppRuntime) -> axum::Router {
    let plugin_context = ui_app::PluginHostContext {
        library_root: runtime.config.library_root().to_path_buf(),
        rest_endpoint: runtime.config.rest_endpoint().to_string(),
    };
    localref_core::rest::router_with_daemon(runtime.daemon.clone()).merge(
        ui_app::router_with_daemon_repo_plugins_and_context(
            runtime.daemon.clone(),
            runtime.config.repo_name().to_string(),
            runtime.plugins.clone(),
            plugin_context,
        ),
    )
}
```
Then `serve_rest_with_runtime` binds the listener and serves `rest_app(runtime)`:
```rust
async fn serve_rest_with_runtime(runtime: &AppRuntime) -> std::io::Result<()> {
    let addr = runtime.config.rest_addr();
    println!("localref REST listening on http://{addr}");
    tracing::info!(target: "localref::rest", "listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, rest_app(runtime)).await
}
```
Delete the old `serve_rest_with_daemon` and the `#[cfg(not(feature = "desktop"))] rest_app` if it is now unreachable (or adapt it the same way — keep the non-desktop variant returning just the core router from `runtime`). For `serve_csc_with_runtime`, inline the daemon clone as in Step 1 (it can keep calling a small `serve_csc_with_daemon(config, daemon)` helper, or be inlined).

- [ ] **Step 4: Build the workspace**

Run: `cargo build -p localref`
Expected: PASS. Remove any leftover unused imports the compiler flags.

- [ ] **Step 5: Run the connector dev binary smoke check**

Run: `cargo build --bin localref-csc-dev`
Expected: PASS (it has its own sink/path and must still compile).

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "refactor: use core-owned connector sink, discover plugins once"
```

---

## Task 9: Render fixed plugin slots concurrently

**Files:**
- Modify: `crates/ui-app/src/server.rs` (`render_fixed_plugin_slots` lines 284-326)

**Context:** Today the function awaits each plugin render in series. We render the matching `(plugin, page)` slots concurrently so total latency is the slowest plugin, not the sum. `invoke_render` already enforces a 30s timeout per call (`invoke.rs:93`) and maps errors to a fallback fragment, so per-slot isolation is preserved — a failing/slow plugin only affects its own slot. **No new dependency:** use `tokio::task::JoinSet` (tokio is already a direct ssr dependency of ui-app). JoinSet handles a dynamic number of tasks, and because each spawned task owns its inputs, there is no borrow-across-await problem. This composes with Task 11 (per-plugin lean state), which moves `build_plugin_ui_state` into each task.

- [ ] **Step 1: Confirm no dependency change is needed**

Run: `grep -n "tokio" crates/ui-app/Cargo.toml`
Expected: `tokio` is present under the `ssr` feature deps. No `Cargo.toml` edit required. (Do NOT add the `futures` crate — `JoinSet` covers this.)

- [ ] **Step 2: Rewrite `render_fixed_plugin_slots` to render concurrently with `JoinSet`**

`PluginSlotHtml` ordering: today slots are pushed in plugin-iteration order. `JoinSet` completes out of order, so we sort the results by `(plugin_name, page_id)` afterward for deterministic SSR output. Replace the body (lines 284-326) with:

```rust
pub async fn render_fixed_plugin_slots(
    model: &mut UiModel,
    state: &ServerState,
) {
    let target_mount = if model.selected_ids.is_empty() {
        PageMount::MetadataPage
    } else {
        PageMount::SelectionPage
    };
    let plugin_state = build_plugin_ui_state(model, state);

    let mut set: tokio::task::JoinSet<crate::model::PluginSlotHtml> =
        tokio::task::JoinSet::new();
    for plugin in state.plugins.iter() {
        for page in plugin
            .manifest
            .pages
            .iter()
            .filter(|page| page.mount == target_mount)
        {
            // Each task owns clones of its inputs (no borrow across await).
            let executable = plugin.executable.clone();
            let page_id = page.id.clone();
            let mount = page_mount_name(&page.mount).to_string();
            let plugin_name = plugin.name().to_string();
            let label = page.label.clone();
            let plugin_state = plugin_state.clone();
            set.spawn(async move {
                let html = match localref_plugin::invoke::invoke_render(
                    &executable,
                    &page_id,
                    &plugin_state,
                )
                .await
                {
                    Ok(output) if output.status == "ok" => output.html,
                    Ok(output) => plugin_error_html(
                        output
                            .message
                            .as_deref()
                            .unwrap_or("plugin render failed"),
                    ),
                    Err(error) => plugin_error_html(&error.to_string()),
                };
                crate::model::PluginSlotHtml {
                    mount,
                    plugin_name,
                    page_id,
                    label,
                    html,
                }
            });
        }
    }

    let mut rendered: Vec<crate::model::PluginSlotHtml> = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(slot) => rendered.push(slot),
            Err(error) => {
                rendered.push(crate::model::PluginSlotHtml {
                    mount: page_mount_name(&target_mount).to_string(),
                    plugin_name: String::new(),
                    page_id: String::new(),
                    label: String::new(),
                    html: plugin_error_html(&error.to_string()),
                });
            }
        }
    }
    // Deterministic order regardless of completion order.
    rendered.sort_by(|a, b| {
        (a.plugin_name.as_str(), a.page_id.as_str())
            .cmp(&(b.plugin_name.as_str(), b.page_id.as_str()))
    });
    model.plugin_slots.extend(rendered);
}
```

Note: `set.spawn` requires the task body be `Send + 'static`; cloning all inputs satisfies this. `PluginUiState` derives `Clone` (`crates/plugin/src/state.rs:6`), so `plugin_state.clone()` is valid. The `JoinSet` join error arm covers a panicking task (extra isolation — a panicking plugin-render task degrades its slot, not the page).

- [ ] **Step 3: Build (ssr)**

Run: `cargo build -p ui-app`
Expected: PASS.

- [ ] **Step 4: Run ui-app tests**

Run: `cargo test -p ui-app`
Expected: PASS (existing tests unaffected — behavior is identical, only concurrency changed).

- [ ] **Step 5: Commit**

```bash
git add crates/ui-app/src/server.rs crates/ui-app/Cargo.toml Cargo.lock
git commit -m "perf: render fixed plugin slots concurrently"
```

---

## Task 10: Add `needs_items` / `needs_active_detail` manifest flags

**Files:**
- Modify: `crates/plugin/src/manifest.rs` (add two fields to `PluginManifest`, default false)
- Modify: `examples/plugins/bibtexer` manifest (`plugin.toml`) — set the flags it needs
- Test: extend the existing `parse_manifest_accepts_cli_and_fixed_page_mounts` test or add a focused one

**Context:** Lean-by-default. The host sends the full `items` list / `active_detail` only when the manifest opts in. Defaults are `false`. `bibtexer` operates on the active item, so it sets `needs_active_detail = true` (and `needs_items` only if it iterates the list — verify by reading its source).

- [ ] **Step 1: Add the fields to `PluginManifest`**

In `crates/plugin/src/manifest.rs`, add to the `PluginManifest` struct (after `pages`, line 21):
```rust
    /// Whether the plugin needs the full visible-items list in its state.
    #[serde(default)]
    pub needs_items: bool,
    /// Whether the plugin needs the active item's detailed metadata.
    #[serde(default)]
    pub needs_active_detail: bool,
```

- [ ] **Step 2: Add a parse test for defaults and opt-in**

Add to the `mod tests` in `manifest.rs`:
```rust
    #[test]
    fn manifest_state_needs_default_false_and_opt_in() {
        let lean = PluginManifest::parse(
            "name = \"lean\"\nexecutable = \"bin/lean\"\n",
        )
        .expect("manifest parses");
        assert!(!lean.needs_items, "needs_items defaults to false");
        assert!(
            !lean.needs_active_detail,
            "needs_active_detail defaults to false",
        );

        let heavy = PluginManifest::parse(
            "name = \"heavy\"\nexecutable = \"bin/heavy\"\nneeds_items = true\nneeds_active_detail = true\n",
        )
        .expect("manifest parses");
        assert!(heavy.needs_items);
        assert!(heavy.needs_active_detail);
    }
```
This guards the wire contract: a missing flag must mean lean (false), and an explicit `true` must be honored. If this defaulted wrong, lean-by-default would silently break every plugin — worth the test.

- [ ] **Step 3: Run the test**

Run: `cargo test -p localref-plugin manifest_state_needs`
Expected: PASS.

- [ ] **Step 4: Set the flags on `bibtexer`**

Read `examples/plugins/bibtexer/plugin.toml` and its `src/main.rs` to see what state it reads. Run: `grep -n "items\|active_detail\|\.state\." examples/plugins/bibtexer/src/main.rs`. Add to `examples/plugins/bibtexer/plugin.toml` the flags it actually uses (e.g. `needs_active_detail = true` if it reads the active item; `needs_items = true` only if it iterates `state.items`). Do not set a flag the plugin does not use.

- [ ] **Step 5: Build the plugin crates and example**

Run: `cargo build -p localref-plugin -p localref-plugin-sdk`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/plugin/src/manifest.rs examples/plugins/bibtexer/plugin.toml
git commit -m "feat: add lean-state opt-in flags to plugin manifest"
```

---

## Task 11: Send lean state by default; include heavy fields only on opt-in

**Files:**
- Modify: `crates/ui-app/src/server.rs` (`build_plugin_ui_state` lines 513-579, and its two call sites: `render_fixed_plugin_slots` and `load_model`)

**Context:** `build_plugin_ui_state` (line 513) always fills `items` and `active_detail`. We make it consult the target plugin's manifest flags. Since one `plugin_state` is currently shared across all slot plugins (line 293), and flags are per-plugin, the cleanest change is to build state *per plugin* with the flags. The `items`/`active_detail` fields are non-optional in `PluginUiState`, so "lean" means an empty `Vec`/`None`, not a schema change (wire-compatible).

- [ ] **Step 1: Add flag parameters to `build_plugin_ui_state`**

Change the signature (line 513) to accept the two flags:
```rust
fn build_plugin_ui_state(
    model: &UiModel,
    server_state: &ServerState,
    needs_items: bool,
    needs_active_detail: bool,
) -> PluginUiState {
```
Inside, gate the two heavy fields. Replace the `items:` initializer (lines 526-545) so it yields `Vec::new()` when `!needs_items`:
```rust
        items: if needs_items {
            model
                .items
                .iter()
                .map(|item| PluginItemSummary {
                    id: item.id.clone(),
                    title: item.title.clone(),
                    authors: item.authors.clone(),
                    item_type: item.item_type.clone(),
                    categories: item.categories.clone(),
                    main_file: item.main_file.clone(),
                    files: item
                        .main_file
                        .iter()
                        .chain(&item.extra_files)
                        .cloned()
                        .collect(),
                })
                .collect()
        } else {
            Vec::new()
        },
```
And gate `active_detail` (lines 556-569):
```rust
        active_detail: if needs_active_detail {
            model.active_metadata.as_ref().map(|doc| PluginActiveDetail {
                metadata_revision: doc.metadata_revision.clone(),
                title: doc.metadata.title.clone(),
                authors: crate::state::author_summary(&doc.metadata),
                item_type: doc.metadata.item_type.clone(),
                year: doc.metadata.year,
                doi: doc.metadata.doi.clone(),
                venue: doc.metadata.venue.clone(),
                language: doc.metadata.language.clone(),
                uri: doc.metadata.uri.clone(),
                abstract_note: doc.metadata.abstract_note.clone(),
            })
        } else {
            None
        },
```
(`categories`, `selected_ids`, `active_id`, repo/status/paths stay — they are cheap and broadly useful.)

- [ ] **Step 2: Update the slot-render loop (Task 9's `JoinSet` function)**

In `render_fixed_plugin_slots`, build per-plugin lean state **before** `set.spawn` (the spawned task is `'static` and cannot borrow `model`/`state`). Remove the shared `let plugin_state = build_plugin_ui_state(model, state);` line near the top. Inside the `for page in ...` loop, before `set.spawn`, add:
```rust
            let plugin_state = build_plugin_ui_state(
                model,
                state,
                plugin.manifest.needs_items,
                plugin.manifest.needs_active_detail,
            );
```
This `plugin_state` is then moved into the spawned task (it is already an owned `PluginUiState`, so no `.clone()` is needed — drop the `let plugin_state = plugin_state.clone();` line from Task 9, since each iteration now produces its own). The rest of the task body (the `invoke_render` match and `PluginSlotHtml` construction) is unchanged.

- [ ] **Step 3: Update the active-tab call site in `load_model`**

In `load_model` (line 258), replace:
```rust
        let plugin_state = build_plugin_ui_state(&model, state);
```
with the active plugin's flags:
```rust
        let plugin_state = build_plugin_ui_state(
            &model,
            state,
            plugin.manifest.needs_items,
            plugin.manifest.needs_active_detail,
        );
```
(`plugin` is already bound from `active_plugin_page` on line 256.)

- [ ] **Step 4: Build (ssr + wasm)**

Run: `cargo build -p ui-app`
Expected: PASS.
Run: `cargo build -p ui-app --target wasm32-unknown-unknown --no-default-features --features hydrate`
Expected: PASS.

- [ ] **Step 5: Run ui-app tests**

Run: `cargo test -p ui-app`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/ui-app/src/server.rs
git commit -m "perf: send lean plugin state unless manifest opts in"
```

---

## Task 12: Full workspace verification gate

**Files:** none (verification only)

- [ ] **Step 1: Workspace build**

Run: `cargo build --workspace`
Expected: PASS, no warnings.

- [ ] **Step 2: Clippy (strict denies)**

Run: `cargo clippy --workspace`
Expected: PASS — `correctness`, `single_call_fn`, `complexity` are deny-by-default; resolve any finding before proceeding.

- [ ] **Step 3: Workspace tests**

Run: `cargo test --workspace`
Expected: PASS, including the two new tests (`unmatched_connector_import_links_under_unmatched_category`, `manifest_state_needs_default_false_and_opt_in`).

- [ ] **Step 4: Full product build**

Run: `python build.py`
Expected: PASS — CSS, WASM, wasm-bindgen, native binary all succeed (the UI changed, so this is the real check).

- [ ] **Step 5: Manual smoke (optional but recommended)**

Run: `cargo run -- ui` (or launch `tray-host`) and confirm the UI loads, a connector import lands under `Cat/unmatched/` when no rule matches, and plugin slots render. Record the result.

- [ ] **Step 6: Final confirmation**

No commit needed (verification only). If all green, the plan is complete.

---

## Spec coverage check

| Spec section | Task(s) |
|---|---|
| §1 Unified `AppRuntime`, mode pruning | 6, 7 |
| §2 Direct connector import, no deferral | 1, 2, 4 (sink relocation) |
| §2 Unmatched → `Cat/unmatched/` | 5 |
| §2 Deferral flow + REST/UI surface deleted | 1, 2, 3 |
| §3 Concurrent slot rendering | 9 |
| §3 Lean state by default | 10, 11 |
| §4 Per-slot isolation, fail-loud bootstrap | 9 (isolation), 6 (`bootstrap` returns Result) |
| §5 Tests: unmatched + per-slot isolation | 5, 9 |
| §5 Delete dead deferral tests | 2 |
| Gate | 12 |
