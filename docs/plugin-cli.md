# Localref Plugin CLI Protocol

A Localref plugin is a standalone CLI binary. The host discovers it via
`plugin.toml`, renders its UI from a static declarative spec in `ui.toml`, and
invokes it with plain argv when the user triggers an action. The plugin talks
back to the daemon over REST to read or write library data.

The same binary and the same argv work identically from a shell or when spawned
by the host — there is no special host-only execution mode.

---

## 1. Plugin identity — `plugin.toml`

Every plugin directory contains a `plugin.toml` that identifies the plugin.

```toml
name        = "bibtexer"
executable  = "bibtexer"
description = "Export citations in BibTeX and RIS formats"
```

| Key          | Type   | Required | Notes |
|--------------|--------|----------|-------|
| `name`       | string | yes      | Machine-readable identifier used in routes and argv |
| `executable` | string | no       | Path to the binary, relative to the plugin directory. Defaults to `name` |
| `description`| string | no       | Human-readable description shown in the UI |
| `ui`         | string | no       | UI-spec filename override. Defaults to `ui.toml` |

A plugin with no `ui.toml` (and no `ui` override that resolves to a file) is
discoverable by the host but exposes nothing in the UI. It is still invocable
from the command line.

### Hooks — `[[hooks]]`

A hook binds the plugin to a daemon lifecycle event. After the event completes,
the host spawns the plugin as `plugin-bin hook <event> …`, fire-and-forget: the
result is logged but never blocks or fails the action that triggered it.

```toml
[[hooks]]
event = "item_imported"

[[hooks]]
event = "item_deleted"
```

| Key     | Type | Required | Values |
|---------|------|----------|--------|
| `event` | enum | yes      | `item_imported` · `category_changed` · `item_deleted` · `metadata_patched` · `scan_completed` |

A plugin may declare any number of hooks. The same plugin can bind to several
events; it dispatches on the `<event>` argument at runtime.

### Cron jobs — `[[cron]]`

A cron job runs the plugin on a schedule. The host spawns it as
`plugin-bin cron <id> …` when due, fire-and-forget.

```toml
[[cron]]
id       = "nightly_sync"
schedule = "0 0 3 * * *"
```

| Key        | Type   | Required | Notes |
|------------|--------|----------|-------|
| `id`       | string | yes      | Job id passed back to the plugin as `cron <id>` |
| `schedule` | string | yes      | Cron expression, six fields: `sec min hour day-of-month month day-of-week` |

There is **no catch-up**: jobs fire only while Localref is running, and the next
fire time is always computed forward from the present. Jobs are skipped while
the daemon is fully paused. Invalid expressions are logged and skipped at
startup; the rest of the plugin still loads.

---

## 2. Declarative UI spec — `ui.toml`

`ui.toml` describes every UI surface the plugin owns. The host renders it
natively — no HTML is produced or consumed by the plugin.

### Actions

An action is a button or context-menu entry that triggers the plugin with no
form. Actions appear in `[[actions]]` tables.

```toml
[[actions]]
id     = "export_ris"
label  = "Export RIS"
mount  = "context_menu"
target = "selection"
```

| Key      | Type   | Required | Values |
|----------|--------|----------|--------|
| `id`     | string | yes      | Action id passed to the binary as `run <id>` |
| `label`  | string | yes      | Display label |
| `mount`  | enum   | yes      | `action_button` · `context_menu` |
| `target` | enum   | no       | `selection` · `active` · `none` (default) |

### Pages

A page is a mounted form. Pages appear in `[[pages]]` tables. Each page can
have any number of `[[pages.fields]]` and `[[pages.display]]` sub-tables.

```toml
[[pages]]
id      = "export"
label   = "Export"
mount   = "selection_page"
route   = "export"
action  = "export_bibtex"
target  = "selection"
preview = { action = "preview_export", debounce_ms = 300, into = "preview_pane" }
```

| Key       | Type        | Required | Notes |
|-----------|-------------|----------|-------|
| `id`      | string      | yes      | Page id |
| `label`   | string      | yes      | Tab or page heading |
| `mount`   | enum        | yes      | `detail_tab` · `metadata_page` · `selection_page` |
| `route`   | string      | yes      | URL route segment |
| `action`  | string      | no       | Action id spawned on form submit |
| `target`  | enum        | no       | `selection` · `active` · `none` (default) |
| `preview` | inline table| no       | See [Live preview](#live-preview) |

#### Fields

Each `[[pages.fields]]` entry declares one form control.

```toml
[[pages.fields]]
name    = "format"
label   = "Format"
kind    = "select"
options = ["bibtex", "ris"]
default = "bibtex"
```

| Key          | Type     | Required | Notes |
|--------------|----------|----------|-------|
| `name`       | string   | yes      | Becomes `--param name=value` on invocation |
| `label`      | string   | yes      | Display label |
| `kind`       | enum     | yes      | `text` · `textarea` · `number` · `checkbox` · `select` · `radio` |
| `options`    | string[] | no       | Option list for `select` and `radio` |
| `default`    | string   | no       | Initial value |
| `required`   | bool     | no       | Defaults to `false` |
| `show_if`    | string   | no       | Reserved — Tier-1 conditional visibility (not yet evaluated) |
| `enabled_if` | string   | no       | Reserved — Tier-1 conditional enabled state (not yet evaluated) |

#### Display readouts

Each `[[pages.display]]` entry declares a live text readout.

```toml
[[pages.display]]
id   = "count"
text = "Exporting {selection.count} item(s)"

[[pages.display]]
id   = "preview_pane"
text = ""
```

| Key    | Type   | Required | Notes |
|--------|--------|----------|-------|
| `id`   | string | yes      | Pane identifier; also the target name for Tier-2 preview |
| `text` | string | yes      | Template text (may be empty for Tier-2 target panes) |

**Tier-1 bindings** are computed in-browser with no plugin call. The host
substitutes tokens in `text` before rendering:

- `{selection.count}` — number of checked items
- `{field.<name>}` — current value of the named form field

**Tier-2 preview** is a debounced plugin call whose plain-text result is
dropped into a named display pane. See below.

#### Live preview

```toml
preview = { action = "preview_export", debounce_ms = 300, into = "preview_pane" }
```

| Key            | Type   | Required | Notes |
|----------------|--------|----------|-------|
| `action`       | string | yes      | Action id spawned to compute the preview |
| `debounce_ms`  | u64    | yes      | Debounce window before the call fires |
| `into`         | string | yes      | Display `id` whose text is replaced with the result |

The preview action must return `{"status":"ok","result":"…"}`. The host strips
the envelope and writes the text into the named pane. Preview results are
text-only; `filename` is ignored for preview calls.

---

## 3. The argv contract

The host spawns the plugin binary with:

```
plugin-bin run <action> --endpoint <url> [--selected a,b,c] [--active id] [--param k=v] …
```

| Segment / flag | Notes |
|----------------|-------|
| `run`          | Literal subcommand |
| `<action>`     | Action id from `ui.toml` |
| `--endpoint`   | Daemon REST base URL (e.g. `http://127.0.0.1:8787`). Build a REST client from this |
| `--selected`   | Comma-separated item ids, present when `target = "selection"` and items are checked |
| `--active`     | Single item id, present when `target = "active"` |
| `--param k=v`  | One flag per form field; repeated for multiple fields |

Each value is a separate argv entry passed directly to the OS spawn API, not
shell-interpolated. Spaces, `=`, and newlines inside a value arrive intact.

`--selected` passes ids as a single comma-separated argument. Item ids use the
format `lr:<connector>:<id>` and never contain commas, so the split is
unambiguous.

`--param` is repeated once per field: `--param format=bibtex --param note=hello world`.

### Target resolution

The host resolves which ids to pass by reading the page or action's `target`
field from `ui.toml`:

- `selection` → `--selected` (comma-joined); `--active` is omitted
- `active`    → `--active`; `--selected` is omitted
- `none`      → neither flag is present

### Standalone shell example

The plugin runs identically from a shell:

```sh
bibtexer run export_bibtex \
    --endpoint http://127.0.0.1:8787 \
    --selected lr:zotero:a,lr:zotero:b \
    --param format=bibtex
```

### Hook and cron invocation

Besides `run`, the host spawns plugins through two more subcommands. Both are
fire-and-forget: the host reads the same result envelope only to log it.

```
plugin-bin hook <event> --endpoint <url> [--item <id>] [--category <path>]
plugin-bin cron <id>    --endpoint <url>
```

| Segment / flag | Notes |
|----------------|-------|
| `hook`         | Literal subcommand |
| `<event>`      | The event name that fired (one of the five hook events) |
| `--item`       | Affected item id, present for item-scoped events (`item_imported`, `item_deleted`, `metadata_patched`, and item-targeted category changes) |
| `--category`   | Affected category path, present for `category_changed` events that name one |
| `cron`         | Literal subcommand |
| `<id>`         | The cron job id declared in `plugin.toml` |
| `--endpoint`   | Daemon REST base URL, as with `run` |

`scan_completed` hooks carry neither `--item` nor `--category`. As with `run`,
the same argv works from a shell:

```sh
hooklog hook item_imported --endpoint http://127.0.0.1:24817 --item lr:zotero:a
hooklog cron heartbeat     --endpoint http://127.0.0.1:24817
```

---

## 4. The result envelope

The plugin prints one JSON object to stdout and exits zero:

```json
{
  "status": "ok",
  "result": "@article{...}",
  "content_type": "text/x-bibtex",
  "filename": "localref-export.bib"
}
```

On error:

```json
{"status": "error", "message": "no items selected"}
```

| Field          | Notes |
|----------------|-------|
| `status`       | `"ok"` or `"error"` |
| `result`       | Text content produced by the action (omitted on error) |
| `content_type` | MIME type of `result` (optional) |
| `filename`     | Suggested save filename. When present the desktop host opens a save dialog and writes `result` to the chosen path |
| `message`      | Error description when `status` is `"error"` |

All fields except `status` are optional and default to null when absent.

For preview calls the host reads only `result`; `filename` and `content_type`
are ignored.

---

## 5. REST access

The plugin receives only item ids on argv — never item data. It fetches and
writes library data through the daemon REST API using the `--endpoint` URL.

Rust plugins can use the `localref-plugin-sdk` crate, which wraps the
`localref-client` crate and exposes `LocalrefClient` along with helpers for
parsing argv and emitting the result envelope. Plugins in other languages make
raw HTTP requests to the same base URL.

### Logging into the unified log

A plugin can write into Localref's unified log (the same JSONL log + in-memory
ring buffer the UI surfaces) with `POST /api/plugins/log`:

```jsonc
// POST <endpoint>/api/plugins/log
{
  "plugin": "bibtexer",          // recorded as target localref::plugin::bibtexer
  "level": "info",               // trace | debug | info | warn
  "message": "exported 3 items",
  "event_kind": "plugin_action", // optional
  "item_id": "lr:zotero:abc",    // optional
  "path": "All/Paper"            // optional
}
```

- The host sanitizes `plugin` to `[a-z0-9_-]` and records the entry under the
  per-plugin target **`localref::plugin::<plugin>`**, so plugin logs are
  filterable on their own.
- The level is **capped at `warn`**: a plugin cannot emit `error` (or any
  higher level), so a misbehaving plugin can't masquerade as a host failure.
  `error` and unrecognized levels are coerced to `warn`.
- Returns `204 No Content`.

From the SDK:

```rust
use localref_plugin_sdk::{LocalrefClient, LogLevel};

let client = LocalrefClient::new(endpoint);
client.log("bibtexer", LogLevel::Info, "exported 3 items").await?;
// …or with structured fields:
client.log_with("bibtexer", LogLevel::Warn, "skipped 1 item",
    Some("plugin_action"), Some("lr:zotero:abc"), None).await?;
```

### Desktop notifications

A plugin can request a desktop notification with `POST /api/notify`:

```jsonc
// POST <endpoint>/api/notify
{
  "title": "Export complete",
  "body": "Wrote 3 BibTeX entries",
  "kind": "success"   // info (default) | success | error
}
```

- Delivery is decoupled: the host enqueues the request and a dedicated thread
  shows it through the native notification layer. The notification is also
  mirrored into the unified log under `localref::notify`.
- Notifications are a **desktop capability**. In a headless build (or any
  process where the notification consumer was never started) the daemon
  responds **`503 Service Unavailable`**; treat that as "not available", not an
  error.
- On success returns `204 No Content`.

From the SDK, `notify` folds the `503` into a boolean so plugins degrade
gracefully:

```rust
use localref_plugin_sdk::{LocalrefClient, NotifyKind};

let client = LocalrefClient::new(endpoint);
let shown = client.notify("Export complete", "Wrote 3 entries", NotifyKind::Success).await?;
// shown == false means the host has no notification capability; carry on.
```

Both calls work from any invocation mode (`run`, `hook`, `cron`) since every
mode receives an `--endpoint`. Hooks and cron jobs are the natural place to use
them — e.g. a nightly cron job logging its summary and notifying on completion.

---

## 6. Worked example — bibtexer

The `examples/plugins/bibtexer/` directory is a complete reference plugin.

### `plugin.toml`

```toml
name        = "bibtexer"
executable  = "bibtexer"
description = "Export citations in BibTeX and RIS formats"
```

No `ui` key, so the host reads `ui.toml` by default.

### `ui.toml`

```toml
[[pages]]
id      = "export"
label   = "Export"
mount   = "selection_page"
route   = "export"
action  = "export_bibtex"
target  = "selection"
preview = { action = "preview_export", debounce_ms = 300, into = "preview_pane" }

[[pages.fields]]
name    = "format"
label   = "Format"
kind    = "select"
options = ["bibtex", "ris"]
default = "bibtex"

[[pages.display]]
id   = "count"
text = "Exporting {selection.count} item(s)"

[[pages.display]]
id   = "preview_pane"
text = ""

[[actions]]
id     = "export_ris"
label  = "Export RIS"
mount  = "context_menu"
target = "selection"
```

This exposes:

- A `selection_page` with a format dropdown, a Tier-1 count readout, and a
  Tier-2 debounced preview pane.
- A context-menu entry that runs `export_ris` directly on the selection.

### `src/main.rs` — dispatch sketch

```rust
match invocation {
    Invocation::Run { action, endpoint, selected, active, params } => {
        let ctx = ActionContext { selected, active, params,
                                  client: LocalrefClient::new(endpoint) };
        emit(&run(&action, &ctx).await);
    }
    Invocation::Manifest => { /* self-check only; host reads plugin.toml */ }
    // Interactive-only plugins still match these arms exhaustively.
    Invocation::Hook { .. } | Invocation::Cron { .. } => {
        emit(&RunOutput::error("bibtexer has no hook or cron entry points"));
    }
}
```

The `run` function dispatches on `action`:

- `"export_bibtex"` — reads `format` from `params`, fetches items via REST,
  returns a `RunOutput` with `filename = "localref-export.bib"`.
- `"export_ris"` — same but forces RIS format and `filename = "localref-export.ris"`.
- `"preview_export"` — calls the same export logic but strips `filename` so the
  host shows the text inline rather than opening a save dialog.

See `examples/plugins/bibtexer/src/main.rs` for the full implementation.

---

## 7. Worked example — hooklog

The `examples/plugins/hooklog/` directory is a reference plugin for the `hook`
and `cron` entry points. It declares two hooks and one cron job, and on every
invocation appends a line to a log file. On hooks it also echoes back to the
daemon — a unified-log entry via `client.log_with(…)` and a desktop
notification via `client.notify(…)` — so the logging and notification paths are
observable end-to-end.

### `plugin.toml`

```toml
name        = "hooklog"
executable  = "hooklog"
description = "Append a line to a log file on each lifecycle hook and cron tick"

[[hooks]]
event = "item_imported"

[[hooks]]
event = "item_deleted"

[[cron]]
id       = "heartbeat"
schedule = "0 * * * * *"
```

### `src/main.rs` — dispatch sketch

```rust
match invocation {
    Invocation::Hook { event, endpoint, item, category } => {
        // append a hook line, then echo to the daemon:
        let client = LocalrefClient::new(&endpoint);
        client.log_with("hooklog", LogLevel::Info, &summary,
            Some(&event), item.as_deref(), None).await.ok();
        client.notify("hooklog", &summary, NotifyKind::Info).await.ok();
    }
    Invocation::Cron { job, .. }   => { /* append a cron line */ }
    Invocation::Run { action, .. } => { /* append a run line  */ }
    Invocation::Manifest           => { /* self-check only    */ }
}
```

The log file defaults to `<temp dir>/localref-hooklog.txt`; set `HOOKLOG_FILE`
to override it. See `examples/plugins/hooklog/src/main.rs` for the full
implementation.
