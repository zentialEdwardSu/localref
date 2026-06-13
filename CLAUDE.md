# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test

```bash
# Full build (CSS, WASM, native binary)
python build.py
python build.py --release

# CSS only
npm run build:css

# Individual crate builds
cargo build -p localref-core
cargo build -p localref-plugin
cargo build -p ui-app --target wasm32-unknown-unknown --no-default-features --features hydrate

# Tests
cargo test                              # all tests
cargo test -p localref-core             # single crate
cargo test <test_name>                  # single test
cargo test -p localref-core <test_name>

# Run (debug)
cargo run -- headless                   # headless server (rest + csc)
cargo run -- rest                       # REST-only diagnostics
cargo run -- ui                         # open browser UI
```

# 12-rules for Implement

These rules apply to every task in this project unless explicitly overridden.
Bias: caution over speed on non-trivial work. Use judgment on trivial tasks.

## Rule 1 — Think Before Coding
State assumptions explicitly. If uncertain, ask rather than guess.
Present multiple interpretations when ambiguity exists.
Push back when a simpler approach exists.
Stop when confused. Name what's unclear.

## Rule 2 — Simplicity First
Minimum code that solves the problem. Nothing speculative.
No features beyond what was asked. No abstractions for single-use code.
Test: would a senior engineer say this is overcomplicated? If yes, simplify.

## Rule 3 — Surgical Changes
Touch only what you must. Clean up only your own mess.
Don't "improve" adjacent code, comments, or formatting.
Don't refactor what isn't broken. Match existing style.

## Rule 4 — Goal-Driven Execution
Define success criteria. Loop until verified.
Don't follow steps. Define success and iterate.
Strong success criteria let you loop independently.

## Rule 5 — Use the model only for judgment calls
Use me for: classification, drafting, summarization, extraction.
Do NOT use me for: routing, retries, deterministic transforms.
If code can answer, code answers.

## Rule 6 — Token budgets are not advisory
Per-task: 4,000 tokens. Per-session: 30,000 tokens.
If approaching budget, summarize and start fresh.
Surface the breach. Do not silently overrun.

## Rule 7 — Surface conflicts, don't average them
If two patterns contradict, pick one (more recent / more tested).
Explain why. Flag the other for cleanup.
Don't blend conflicting patterns.

## Rule 8 — Read before you write
Before adding code, read exports, immediate callers, shared utilities.
"Looks orthogonal" is dangerous. If unsure why code is structured a way, ask.

## Rule 9 — Tests verify intent, not just behavior
Tests must encode WHY behavior matters, not just WHAT it does.
A test that can't fail when business logic changes is wrong.

## Rule 10 — Checkpoint after every significant step
Summarize what was done, what's verified, what's left.
Don't continue from a state you can't describe back.
If you lose track, stop and restate.

## Rule 11 — Match the codebase's conventions, even if you disagree
Conformance > taste inside the codebase.
If you genuinely think a convention is harmful, surface it. Don't fork silently.

## Rule 12 — Fail loud
"Completed" is wrong if anything was skipped silently.
"Tests pass" is wrong if any were skipped.
Default to surfacing uncertainty, not hiding it.

## Architecture

Localref is a tray-resident desktop reference manager. The single binary (`localref`) starts as a tray-hosted daemon providing REST + Zotero Connector APIs and a Leptos web UI. The filesystem is the source of truth; the redb query database is always rebuildable.

**Runtime modes** (`src/main.rs`):
- `tray-host` (default): tray icon + daemon + REST + CSC + UI
- `headless` / `serve`: REST + CSC servers, no tray
- `rest`: REST-only for diagnostics
- `csc` / `csc-dev`: connector API only
- `ui`: open the browser-served web UI
- `tray <action>`: drive the tray from CLI (open-ui, scan, pause/resume, quit)

**Workspace crates:**

| Crate | Purpose |
|---|---|
| `localref-core` (`crates/core`) | Import pipeline, daemon task queue, redb query storage, REST API endpoints, rules engine, event log, lock management |
| `csc` (`crates/csc`) | Zotero Connector HTTP protocol adapter — an Axum server that accepts connector-shaped requests and forwards them to a `ConnectorImportSink` trait |
| `localref-plugin` (`crates/plugin`) | Plugin discovery (scans directories for `plugin.toml`), invocation via stdio JSON subprocess calls, shared types (`RenderOutput`, `RunOutput`, `PluginUiState`) |
| `localref-plugin-sdk` (`crates/plugin-sdk`) | Optional Rust trait (`Plugin`) for building plugins more ergonomically than raw stdio JSON |
| `ui-app` (`crates/ui-app`) | Leptos SSR + WASM hydration web UI (`ssr` feature for server, `hydrate` feature for wasm32). Provides `router_with_daemon_repo_plugins_and_context()` to merge plugin routes into the Axum router |
| `native-win32` (`crates/native-win32`) | Windows platform utilities (open URI, detach console) |

**Library filesystem layout:**
```
~/.localref/libroot/
  All/                    # item directories, each with metadata.toml + files
  Cat/                    # category hierarchy with symlinks into All/
  .localref/
    db/query.redb         # redb rebuildable query database
    rules.toml            # auto-classification rules
    locks/                # filesystem locks
    logs/events.jsonl     # daemon event log
```

**Key abstractions in `localref-core`:**
- `LocalrefDaemon` — the main facade; owns task queue, storage, event log, pending imports
- `ImportPipeline` — stateless pipeline for writing items into `All/` with metadata
- `StorageDb` — redb-backed query index rebuildable from `All/*/metadata.toml`
- `LibraryFs` — filesystem helpers: atomic writes, NTFS-safe filenames, category link management
- `RuleSet` — parses `rules.toml` and matches imported metadata against automatic classification rules
- `EventLog` — append-only JSONL event log for observable daemon activity
- `LockManager` — cross-process filesystem lock files under `.localref/locks/`

**Plugin system:**
Plugins are standalone CLI binaries. The host discovers them by scanning `plugin.toml` files, then invokes them via stdin/stdout JSON protocol. See `docs/plugin-cli.md` for the full protocol. Three request modes:
- `manifest` — return capabilities (actions, pages, mount points)
- `render` — return an HTML fragment for a page mount
- `run` — execute an action, return structured output (optionally with `result` + `filename` for save dialogs)

The web UI mounts plugins at `/plugin/<name>/action` for form POSTs. Plugin pages integrate at mount points: detail tabs, metadata pages, and selection pages.

**UI architecture (`ui-app`):**
- `app.rs` — Leptos component tree
- `route.rs` — `RouteState` parsed from query string (q, category, active, tab, selected)
- `server.rs` — Axum handlers serving SSR HTML and API endpoints for plugin actions, file uploads, metadata patches
- `model.rs` — `UiState` and serializable types shared between server render and hydration
- `client.rs` — browser hydration entry point
- `assets.rs` — embedded static asset serving

**Build pipeline (`build.py`):**
1. `npm run build:css` — Tailwind CSS
2. `cargo build -p ui-app --target wasm32-unknown-unknown --features hydrate` — WASM UI
3. `wasm-bindgen` — generates JS bindings into `assets/`
4. `cargo build -p localref` — native binary (embeds assets)

## Feature flags (root `Cargo.toml`)

- `desktop` (default) — includes `ui-app` with SSR, enables REST+CSC+UI
- `native-tray` (default) — Windows native tray icon via `tao` + `tray-icon`
- Build without defaults for a headless server: `cargo build --no-default-features`

## Conventions

- Edition 2024 throughout the workspace
- Clippy `correctness`, `single_call_fn`, and `complexity` are deny-by-default; `pedantic` is warn
- Filesystem paths use forward slashes in code; `platformfs` handles NTFS sanitization on Windows
- Daemon tasks flow through the `LocalrefDaemon` queue — always use the typed methods (`import_connector_item`, `patch_metadata`, etc.) rather than `execute_task` directly
- Metadata edits require a revision hash for optimistic concurrency; on conflict the candidate is saved as `metadata.daemon.toml` alongside the original
