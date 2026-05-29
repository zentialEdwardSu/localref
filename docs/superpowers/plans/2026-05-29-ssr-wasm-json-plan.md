# SSR WASM JSON UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the first migration slice for a server-rendered shell with JSON UI state and a Rust/WASM client controller.

**Architecture:** The server keeps rendering `/` from `UiModel`, and adds `/ui/state` as the stable browser data contract. A new `crates/ui-wasm` crate owns route/query state and fetches this contract; Tasks 3 and 4 connect it to the served browser assets.

**Tech Stack:** Rust 2024, Axum, Leptos SSR, Serde, wasm-bindgen, web-sys, wasm-bindgen-test.

---

### Task 1: JSON UI State Contract

**Files:**
- Create: `crates/ui-web/src/dto.rs`
- Modify: `crates/ui-web/src/lib.rs`
- Modify: `crates/ui-web/Cargo.toml`

- [x] **Step 1: Write the failing server tests**

Add tests in `crates/ui-web/src/lib.rs` that request `/ui/state?q=alpha`, parse JSON, and assert the response contains the filtered item, active id, and tab. Also add a direct DTO conversion test that proves selected ids and common state survive conversion.

- [x] **Step 2: Run the focused test and verify failure**

Run: `cargo test -p ui-web ui_state`

Expected: FAIL because `/ui/state` and `dto` do not exist.

- [x] **Step 3: Add `dto.rs`**

Create documented DTO structs with `Serialize`: `UiStateDto`, `ItemSummaryDto`, `CategorySummaryDto`, `FileEntryDto`, `EventDto`, and `RulesNoticeDto`. Implement `UiStateDto::from_model(model: UiModel)`.

- [x] **Step 4: Add the Axum route**

Expose `GET /ui/state` from `router_with_daemon`, load `UiModel` with `Query<UiQuery>`, and return `Json(UiStateDto::from_model(model))`.

- [x] **Step 5: Run tests**

Run: `cargo test -p ui-web ui_state`

Expected: PASS.

- [ ] **Step 6: Commit**

Run: `git add crates/ui-web/src/dto.rs crates/ui-web/src/lib.rs crates/ui-web/Cargo.toml Cargo.toml Cargo.lock docs/superpowers/plans/2026-05-29-ssr-wasm-json-plan.md && git commit -m "feat: add ui state json contract"`

### Task 2: WASM Crate Skeleton And Query State

**Files:**
- Create: `crates/ui-wasm/Cargo.toml`
- Create: `crates/ui-wasm/src/lib.rs`
- Create: `crates/ui-wasm/src/query.rs`
- Modify: `Cargo.toml`

- [x] **Step 1: Write query tests**

Add unit tests in `query.rs` for constructing route strings from search, category, active item, selected ids, and tab.

- [x] **Step 2: Run focused test and verify failure**

Run: `cargo test -p ui-wasm query`

Expected: FAIL until the crate and query module exist.

- [x] **Step 3: Add crate and query module**

Create `RouteState` with documented fields and methods for parsing from query pairs and serializing into a query string.

- [x] **Step 4: Run focused test**

Run: `cargo test -p ui-wasm query`

Expected: PASS.

- [ ] **Step 5: Commit**

Run: `git add Cargo.toml Cargo.lock crates/ui-wasm && git commit -m "feat: add wasm ui route state"`

### Task 3: WASM API Client Boundary

**Files:**
- Create: `crates/ui-wasm/src/api.rs`
- Modify: `crates/ui-wasm/src/lib.rs`
- Modify: `crates/ui-wasm/Cargo.toml`

- [x] **Step 1: Add serializable client DTOs**

Mirror only the fields needed by Phase 1: items, categories, active id, selected ids, tab, return path, status label, and rules notice.

- [x] **Step 2: Add fetch function**

Implement `fetch_state(route: &RouteState)` as an async WASM function that calls `/ui/state?<query>` and deserializes JSON.

- [x] **Step 3: Build the WASM crate**

Run: `cargo check -p ui-wasm --target wasm32-unknown-unknown`

Expected: PASS once the wasm target exists locally.

- [ ] **Step 4: Commit**

Run: `git add crates/ui-wasm Cargo.toml Cargo.lock && git commit -m "feat: add wasm ui state client"`

### Task 4: Bootstrap Asset Boundary

**Files:**
- Create: `crates/ui-web/src/wasm_assets.rs`
- Modify: `crates/ui-web/src/lib.rs`
- Modify: `crates/ui-web/src/components/mod.rs`

- [x] **Step 1: Add route tests for bootstrap assets**

Assert `/assets/localref-ui.js` is served as JavaScript and contains only WASM bootstrap code, not Localref routing business logic.

- [x] **Step 2: Add asset routes**

Serve `/assets/localref-ui.js` and `/assets/localref-ui.wasm`. The JS bootstrap imports and initializes the WASM module.

- [ ] **Step 3: Replace inline script tag**

Remove `<script>{INTERACTION_SCRIPT}</script>` from SSR output and load the bootstrap script with `type="module"`.

- [x] **Step 4: Run web tests**

Run: `cargo test -p ui-web`

Expected: PASS after updating tests that currently assert inline script content.

- [ ] **Step 5: Commit**

Run: `git add crates/ui-web Cargo.toml Cargo.lock && git commit -m "feat: load wasm ui bootstrap"`

### Task 5: Browser Flow Verification

**Files:**
- Modify tests only if a browser harness already exists.

- [ ] **Step 1: Build the project**

Run: `cargo test -p ui-web && cargo check -p ui-wasm --target wasm32-unknown-unknown`

Expected: PASS.

- [ ] **Step 2: Run the app manually or through the existing dev binary**

Run: `cargo run --bin localref-rest-dev`

Expected: the server starts and serves the SSR page.

- [ ] **Step 3: Verify migrated flows**

Open the UI and verify search, category filter, active item, tab, selection, and back/forward still work without a full reload for the parts implemented in this phase.
