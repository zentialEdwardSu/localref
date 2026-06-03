# Localref Plugin CLI Protocol

Plugins are standalone CLI programs. Localref sends one JSON request to stdin
and reads one JSON response from stdout. A plugin can be written in any
language as long as it implements this process contract.

## Manifest

Each plugin directory contains `plugin.toml`.

```toml
name = "bibtexer"
executable = "bibtexer"
description = "Export citations"

[[actions]]
id = "export_bibtex"
label = "BibTeX"
mount = "action_button"

[[pages]]
id = "metadata_export"
label = "Citation Export"
mount = "metadata_page"
route = "metadata-export"
```

`executable` is relative to the plugin directory. If it is omitted, Localref
looks for an executable named after `name`.

Supported action mounts are:

- `action_button`
- `context_menu`

Supported page mounts are:

- `detail_tab`
- `metadata_page`
- `selection_page`

## Requests

Render requests produce HTML for a page mount:

```json
{"mode":"render","page":"metadata_export","state":{}}
```

Action requests process host or plugin page interaction:

```json
{"mode":"run","action":"export_bibtex","params":{"format":"bibtex"},"state":{}}
```

The `state` object contains the current repository, visible items, selected
ids, active metadata, library root, and REST endpoint. The plugin should treat
it as input data and return either structured action output or an HTML fragment.

## Responses

Render responses:

```json
{"status":"ok","html":"<form>...</form>"}
```

Action responses:

```json
{
  "status": "ok",
  "result": "@article{...}",
  "content_type": "text/x-bibtex",
  "filename": "localref-export.bib"
}
```

Mounted HTML can post forms to `/plugin/<name>/action`. Use `plugin_action`
or `action` as the action field, plus any additional parameters. Localref
invokes the plugin CLI again and passes those parameters in `params`. When an
action returns `result` with a `filename`, the desktop host asks the user for a
save path and writes the result to that file.
