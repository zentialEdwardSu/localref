# SSR Shell With Rust/WASM Client Controller

## Context

Localref's current web UI renders the initial document on the server with
Leptos SSR. The page then uses an inline JavaScript string to intercept
navigation and form submissions, fetch rendered HTML, parse it with
`DOMParser`, replace `.app-shell`, and update browser history.

The desired direction is to keep the no-refresh user experience while moving
browser-side business logic out of hand-written inline JavaScript and into Rust
compiled to WebAssembly. The architecture may also change from HTML fragment
replacement to structured JSON APIs plus client-side rendering.

## Goals

- Keep server-rendered first load for direct URLs and tray-launched pages.
- Preserve no-refresh interactions for library browsing.
- Move browser-side UI logic to Rust/WASM.
- Replace HTML-shell fetch/parse/replace routing with JSON state APIs for the
  first migration slice.
- Keep the first phase narrow enough to verify without rewriting the whole UI.

## Non-Goals

- Do not convert the whole app to a client-only SPA.
- Do not require full Leptos hydration in the first phase.
- Do not remove server-rendered page output.
- Do not rewrite unrelated core daemon, storage, or tray behavior.

## Recommended Architecture

Use a server-rendered shell plus a Rust/WASM client controller.

The existing `/` route remains responsible for loading `UiModel` and rendering
the complete first HTML document. The rendered markup must still reflect URL
query state such as search text, category, selected item ids, active item, and
active tab.

Add a structured JSON state route, `/ui/state`, that accepts the same query
parameters as `/` and returns a serializable UI state DTO. The DTO is the
browser contract. It should be derived from `UiModel`, but it should not expose
internal core types directly when a smaller view-specific shape is clearer.

Add a Rust/WASM crate, `crates/ui-wasm`, that owns browser-side interaction
logic:

- Read current URL query state.
- Call `/ui/state` after query-changing interactions.
- Update browser history with `pushState` and respond to `popstate`.
- Render the migrated dynamic regions from JSON state.
- Submit migrated actions through HTTP and refresh state from the returned or
  current route state.

The browser will still need a minimal JavaScript bootstrap to load the WASM
module. That bootstrap must not contain Localref UI business logic.

## Phase 1 Scope

Migrate browsing interactions first:

- Search query changes.
- Category filter changes.
- Active item changes.
- Metadata/files/rules tab changes.
- Selection checkbox changes.
- Browser back and forward navigation.

Keep category mutation, metadata save, rules save, and local file actions on the
existing server form/action path until the browsing state migration is stable.
Those actions can be moved in later phases after the JSON state and WASM render
loop are proven.

## Server Components

`UiModel` remains the server-side assembly point.

Add a DTO module under `crates/ui-web/src`, for example `dto.rs`, with
documented serializable types:

- `UiStateDto`
- `ItemSummaryDto`
- `CategorySummaryDto`
- `DetailDto`
- `FileEntryDto`
- `EventDto`
- `RulesNoticeDto`

Add `UiStateDto::from_model(model: UiModel)` or equivalent conversion. This
conversion should be unit tested because it defines the frontend contract.

Add `/ui/state` to the Axum router:

- Input: `Query<UiQuery>`
- Output: JSON DTO or a clear error response
- Behavior: load the same model as `/`, then serialize the DTO

## WASM Components

The WASM crate should use `wasm-bindgen` and `web-sys` directly for the first
phase. Avoid adding a full UI framework until there is a concrete need.

Suggested modules:

- `lib.rs`: exported initialization entry point and panic hook setup.
- `query.rs`: URL query parsing and route construction.
- `api.rs`: fetch `/ui/state` and decode JSON.
- `render.rs`: render migrated DOM regions from DTOs.
- `events.rs`: bind browser event listeners.
- `state.rs`: hold current DTO and route state.

Every public module and exported function should have documentation.

## Rendering Boundary

Phase 1 should render only the migrated interactive regions from WASM. The SSR
HTML should remain structurally compatible so the initial document can be read
before WASM starts.

The first implementation can replace specific containers rather than attempting
fine-grained DOM reconciliation. Container replacement is acceptable if it is
driven by structured DTOs instead of parsing full HTML documents.

## Error Handling

State fetch failures should fail loud in development and visibly preserve the
current UI state. Do not add silent fallback behavior that hides broken API
contracts.

Action migration should not begin until the browsing state flow has tests. When
actions are migrated, action errors should be represented in JSON and rendered
through explicit UI state rather than inferred from redirect URLs.

## Testing

Unit tests:

- DTO conversion from representative `UiModel` values.
- Query state parsing and serialization.
- Server `/ui/state` response for search, category, active item, selected ids,
  and tab state.

Browser-level test:

- Load `/`.
- Change search without a full document navigation.
- Select an item and switch tabs without a full document navigation.
- Use back/forward and verify visible state matches the URL.

Existing SSR tests should remain and continue to prove direct URLs render
usable first-load HTML.

## Success Criteria

- No Localref UI business logic remains in the inline `INTERACTION_SCRIPT` for
  the migrated browsing interactions.
- The first-load page is still rendered by the server from URL query state.
- Search, category filter, active item, tab, selection, and back/forward work
  without a full page reload.
- `/ui/state` is covered by Rust tests.
- WASM query and rendering logic is covered by focused tests where practical.

## Open Follow-Up

After Phase 1 passes, migrate server actions in small slices:

1. Category add/remove/create.
2. Rules save and result notice.
3. Metadata save with revision handling.
4. Local file open/add/import actions.
