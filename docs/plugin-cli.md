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
}
```

The `run` function dispatches on `action`:

- `"export_bibtex"` — reads `format` from `params`, fetches items via REST,
  returns a `RunOutput` with `filename = "localref-export.bib"`.
- `"export_ris"` — same but forces RIS format and `filename = "localref-export.ris"`.
- `"preview_export"` — calls the same export logic but strips `filename` so the
  host shows the text inline rather than opening a save dialog.

See `examples/plugins/bibtexer/src/main.rs` for the full implementation.
