# Architecture Rethink — Unified Runtime, Direct Connector Imports, Lean Plugins

**Date:** 2026-06-13
**Status:** Design approved, ready for implementation planning
**Scope:** Approach B — consolidate app wiring + optimize the plugin system. Wire-compatible with existing plugins. No protocol redesign.

## Problem

The app "feels disjointed, like several parts were crudely glued together." The root cause: `src/main.rs` does architecture's job. It opens the daemon three different ways, owns connector-import buffering logic that belongs in core, merges routers, and starts servers asymmetrically per runtime mode. The plugin system compounds this — it spawns a fresh subprocess per render/action, sequentially, serializing the full UI state every time.

This design fixes the structural seams and optimizes plugin invocation host-side, without changing the on-wire plugin protocol or the filesystem-as-source-of-truth model.

## Goals

- One daemon-open path shared by every runtime mode.
- Connector imports flow `csc → core → All/` directly, finalized immediately — no user-deferral step.
- Nothing is ever orphaned: every item lands in `All/` and is linked under at least one `Cat/` category.
- Plugin-rendered pages render fixed slots concurrently and send lean state by default.
- Preserve existing behavior for users; keep existing plugins working.

## Non-Goals

- No plugin protocol redesign (stringly-typed JSON stays — that was Approach C).
- No persistent plugin workers. Plugins remain standalone spawn-per-call CLIs (see Section 3 rationale).
- No hot-reload of plugins.
- No language/framework change. Single tray-resident binary stays.

---

## Section 1 — Unified runtime (`AppRuntime`)

### Today
`src/main.rs` opens the daemon three ways: `open_daemon()` (tray path), a hand-built `StorageDb`+`LocalrefDaemon` in `serve_rest`, and `LocalrefDaemon::for_library()` in `serve_csc_only`. Config is cloned per mode. Startup is asymmetric — tray mode hides server startup in a background thread; headless awaits directly.

### Change
One constructor every mode goes through:

```rust
struct AppRuntime {
    config:  LocalrefConfig,        // loaded once, owned here
    daemon:  Arc<LocalrefDaemon>,   // opened ONE way
    plugins: Arc<Vec<DiscoveredPlugin>>,
}

impl AppRuntime {
    fn bootstrap(config: LocalrefConfig) -> Result<Self, LocalrefError>;
}
```

Each mode becomes a thin function taking `&AppRuntime` that decides *which servers to start*, not *how to build the daemon*.

### Runtime modes after pruning

| Mode | Kept? | Servers |
|---|---|---|
| `tray-host` (default) | ✅ | REST + CSC + UI (background) + tray |
| `ui` | ✅ | servers + open browser |
| `tray <action>` | ✅ | drives running instance via CLI |
| `headless` / `serve` | ❌ removed | — |
| `rest` (REST-only diagnostics) | ❌ removed | — |
| `csc` (in-`main` CSC-only diagnostics) | ❌ removed | — |

`AppCommand` reduces to `TrayHost`, `Ui`, `Tray { action }`. The standalone **`src/bin/localref-csc-dev.rs`** binary **stays** — it is the direct Zotero-connector check tool and is independent of the in-`main` `Csc` mode being removed.

### Result
- The three daemon-open paths collapse to one `bootstrap`.
- `serve_rest` / `serve_csc_only` and their bespoke daemon construction are deleted.
- Config is a single owned artifact passed by reference.
- `main.rs` returns to: parse args, bootstrap, dispatch. No daemon construction, no buffering, no router-merge logic.

### Implication (accepted)
Removing `headless` means there is no tray-less way to run the full server from the main binary. For a Windows tray-resident desktop app this is acceptable; there is no remaining headless escape hatch.

---

## Section 2 — Connector imports: one direct path, no deferral

### Today
Two pending-import systems exist:
- Core's `PendingImportStore` (`crates/core/src/pending.rs`) — the user-deferral / category-confirmation flow.
- `src/main.rs`'s `LoggingImportSink` — a *separate* buffer that assembles a connector item with its late-arriving attachments across follow-up requests, then imports.

"What is a pending import?" has two answers in two layers, and the CSC protocol adapter in `main.rs` carries real import logic.

### Change
Collapse to one direct path: `csc → core.import_connector_item → written to All/`, finalized immediately. The rules engine assigns categories automatically. No human-in-the-loop deferral.

**Unmatched-import rule:** in the category-linking path (`import_connector_item_with_categories`, `crates/core/src/lib.rs:1505`):
- `RuleSet::match_metadata` returns categories → use them.
- Returns empty → substitute `vec![CategoryPath::new(UNMATCHED_CATEGORY)]` where `UNMATCHED_CATEGORY = "unmatched"` (a named constant, not a magic string).

Every item is **always** written to `All/` (unconditional source of truth) and **always** linked under at least one `Cat/` category — real categories when matched, `Cat/unmatched/` otherwise. Nothing is ever orphaned. The `if !categories.is_empty()` guard at `lib.rs:1584` becomes dead (categories is now always non-empty) and is removed.

### Deleted (user-deferral flow)
- `crates/core/src/pending.rs` — entire module.
- `LocalrefDaemon`: the `pending` field; `create_pending_connector_import`, `pending_imports`, `confirm_pending_import`, `cancel_pending_import`.
- Re-exports in `lib.rs`: `PendingImportConfirmation`, `PendingImportSession`, `PendingImportStore`.
- REST: routes `/api/import/pending`, `/api/import/pending/{id}/confirm`, `/api/import/pending/{id}/cancel`; handlers `pending_imports`, `confirm_pending_import`, `cancel_pending_import`; the `confirms_pending_imports` test.
- UI: `pending_count` field and the "{n} pending" header indicator, threaded through `crates/ui-app/src/{state.rs,model.rs,dto.rs,app.rs}`.
- `LogKind::ImportPendingUserConfirmation`.
- `dashboard_snapshot.pending_count` and the `list_pending_imports` REST-client call behind it (`src/rest_client.rs`).

### Relocated, not removed
The `LoggingImportSink` connector-session **assembly** buffering (match an incoming item to its open session, hold until attachments/metadata arrive, then finalize) moves from `src/main.rs` into `crates/core`, behind the `csc` crate's existing `ConnectorImportSink` trait. This buffer assembles a complete wire payload over milliseconds — it never waits on a human.

After this, `main.rs` keeps only: wire the csc server to the core-owned sink. Core owns the connector lifecycle end to end. The `ConnectorImportSink` trait boundary stays — only the *implementation* moves to the correct side of it.

### Note on distinctness (verified)
The `main.rs` sink buffering and the core `PendingImportStore` are genuinely different concerns. Only the latter (user-deferral) is removed. The former (protocol assembly) is preserved and relocated. They were never the same mechanism.

---

## Section 3 — Plugin invocation: standalone CLIs, concurrent slots, lean state

### Core principle (overrides any latency optimization)
A plugin is a CLI: invoked, does one job, exits. It must be runnable standalone from a shell with identical behavior to host invocation. The stdio JSON protocol and mount points are tradeoffs layered *on* the standalone-CLI core, not replacements for it. Persistent workers would turn a plugin into a daemon — a different contract that breaks "act standalone" — and are therefore **rejected**.

### Today
Every render/action spawns a fresh subprocess, sends the entire `PluginUiState` (full items list + active detail) on stdin, waits, parses stdout. A page with 3 plugin slots spawns 3 processes **sequentially**.

### Change — two host-side optimizations, both wire-compatible
The on-wire JSON request/response is **unchanged**. Spawn-per-call is **kept**. Existing plugins keep working without modification.

**Optimization A — Concurrent slot rendering.** `render_fixed_plugin_slots` (`crates/ui-app/src/server.rs`) renders all fixed slots concurrently (join the futures) instead of sequentially. Page-render latency becomes the *slowest* plugin, not the *sum*. Each slot gets a timeout with a fallback error fragment, so one slow/failed plugin degrades only its own slot, not the whole page. Still one subprocess per slot — spawned in parallel rather than in series.

**Optimization B — Lean state by default.** Stop sending the full `items` list and `active_detail` to every plugin. The manifest declares what each plugin needs:
- `needs_items` (default `false`)
- `needs_active_detail` (default `false`)

The host sends a minimal `PluginUiState` by default and includes the fat fields only when declared. Lean-by-default: an existing third-party plugin that silently relied on `items` will receive an empty list until its manifest opts in (a one-line addition). `bibtexer` and the SDK get the correct flags set as part of this work.

### Deliberately not doing
Persistent worker pool (violates standalone-CLI principle); protocol/schema redesign (Approach C); hot-reload. Out of scope for B.

---

## Section 4 — Error handling & observability

Preserve existing behavior; the rethink must not regress it.

- **Plugin failures stay isolated.** Per-slot timeout + fallback fragment means a crashed/slow plugin renders an error in its own slot only. The existing `plugin_error_html` path stays; it becomes per-slot instead of page-fatal.
- **One error type per boundary, unchanged.** `LocalrefError` in core, `PluginError` in the plugin crate, `String` at the connector-sink trait boundary. `AppRuntime::bootstrap` returns `Result<Self, LocalrefError>` — no new error type introduced.
- **`bootstrap` fails loud.** If the daemon can't open (bad library root, redb failure), `bootstrap` returns `Err` and `main` exits with a clear message — no silent degraded mode. This replaces today's scattered `.expect()` calls in per-mode setup.
- **Event log unchanged**, minus the deleted `ImportPendingUserConfirmation` kind. Connector imports still emit existing import events; the `unmatched` fallback emits a normal "matched 1 category" event (category = `unmatched`), so it is visible like any classification.

---

## Section 5 — Testing

Per project Rule 9 (tests verify intent, not just behavior) and Rule 2 (nothing speculative): test only what is new, easy to get wrong, or fails silently. Skip tests that would essentially always pass.

### Worth testing (failure is silent or costly)
- **Unmatched import → `Cat/unmatched/`.** New behavior and the "nothing is ever orphaned" guarantee. If it regresses, items vanish from the category tree with no error. Direct test: import an item that matches no rule, assert it exists in `All/` *and* is linked under `Cat/unmatched/`.
- **Per-slot plugin failure isolation.** A timing-out/failing plugin must not blank the whole page. Test that one failing slot yields a fallback fragment while sibling slots still render.

### Not worth a dedicated test (would likely always pass)
- Timing assertion (render ≈ slowest, not sum) — flaky and tautological once the join is in place.
- Lean-state flag plumbing (plugin without `needs_items` gets empty list) — a struct-field branch, covered implicitly.
- `bootstrap` "opens the daemon once" — low-risk; the existing REST/CSC integration tests passing against the unified runtime is the real evidence.

### Maintenance (not new tests)
- Delete `confirms_pending_imports` and the REST pending tests — the feature is removed; leaving them is dead/failing code.
- Keep existing connector-import-with-rules tests green (they guard that matched imports still classify correctly).

### Gate
`cargo build --workspace`, `cargo clippy` (strict `complexity` / `single_call_fn` denies), `cargo test --workspace` all green. Then full `python build.py` (CSS + WASM + native) as the manual UI check, since the UI changed.

---

## Summary of changes by file

| Area | Files | Change |
|---|---|---|
| Runtime | `src/main.rs` | Add `AppRuntime::bootstrap`; reduce `AppCommand` to 3 variants; delete `serve_rest`/`serve_csc_only`; remove `LoggingImportSink` |
| Connector | `crates/core/src/lib.rs`, new sink impl in core | Move session-assembly sink into core behind `ConnectorImportSink`; add `unmatched` fallback; delete deferral methods |
| Connector | `crates/core/src/pending.rs` | Delete module |
| REST | `crates/core/src/rest.rs` | Delete 3 pending routes + handlers + test |
| UI | `crates/ui-app/src/{state.rs,model.rs,dto.rs,app.rs}` | Remove `pending_count` + "{n} pending" indicator |
| Client | `src/rest_client.rs` | Remove `list_pending_imports`, `dashboard_snapshot.pending_count` |
| Plugins | `crates/ui-app/src/server.rs` | Concurrent fixed-slot rendering + per-slot timeout/fallback |
| Plugins | `crates/plugin/src/{manifest.rs,state.rs}` | Add `needs_items`/`needs_active_detail` flags; lean state assembly |
| Plugins | `crates/plugin-sdk`, `examples/plugins/bibtexer` | Set the new manifest flags |
