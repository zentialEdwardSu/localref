# Localref

> [!WARNING]
>
> **Agent has finished it's work, and manual code review and code cleaning are currently underway**

## Runtime Model

The normal user-facing entry is `localref` with no subcommand. It starts a
tray-resident desktop daemon, not a system service:

```text
localref
  tray icon process
    daemon runtime
    REST API
    Zotero Connector API
    Simple UI window on demand
```

Closing the tray process ends the daemon. `localref headless` is available for
diagnostics when a tray icon is not wanted.

## Configuration

Localref reads runtime configuration from the TOML file named by
`LOCALREF_CONFIG`. If `LOCALREF_CONFIG` is not set, the default path is
`~/.localref/config.toml`. If the file does not exist, Localref creates it with
the defaults below.

```toml
library_root = "C:/Users/you/.localref/libroot"

[rest]
addr = "127.0.0.1:24817"
endpoint = "http://127.0.0.1:24817"

[csc]
addr = "127.0.0.1:23119"
```

`rest.addr` is used by the REST server. `rest.endpoint` is used by desktop
clients such as the simple UI and tray. There is no separate UI command:
`localref ui` opens the Simple UI in the same executable.

## Automatic Classification Rules

Import-time classification rules live at `library/.localref/rules.toml`, where
`library` is the configured `library_root`. The web UI also exposes a simple
Rules editor for this file.

Each rule is a TOML array table with three fields:

```text
[[rules]]
name   = human-readable label
target = category path relative to Cat/
query  = one query expression evaluated against one imported metadata record
```

The query grammar is intentionally small:

```text
query   = atom (" OR " atom)*
atom    = field ":" matcher
matcher = substring | "/" regex "/" flags
flags   = "" | "i"
```

`OR` must be uppercase and surrounded by one space on each side. There is no
`AND`, grouping, negation, precedence, phrase operator, or cross-item query.
Empty fields and empty matchers are rejected.

Supported fields are `title`, `abstract` or `abstract_note`, `doi`, `uri` or
`url`, `type` or `item_type`, `venue`, `year`, and `tags` or `tag`. Unknown
fields are valid syntax but never match.

Plain substring matchers are case-insensitive:

```toml
query = 'title:near field'
```

Regex matchers use Rust `regex` syntax. They are case-sensitive by default and
become case-insensitive with the `i` flag:

```toml
query = 'title:/near[- ]field/i'
```

A rule matches when any atom in its `OR` expression matches. Matching rules add
their `target` category. Duplicate target categories are returned once, keeping
the first matching rule order.

Example rules:

```toml
[[rules]]
name = "near-field"
target = "Wireless/RIS"
query = 'title:/near[- ]field/i OR abstract:channel OR tags:RIS'

[[rules]]
name = "doi-prefix"
target = "Publishers/IEEE"
query = 'doi:10.1109'

[[rules]]
name = "webpage"
target = "Sources/Web"
query = 'type:webpage OR uri:/^https?:\/\//i'
```

Expected effects:

- `near-field` adds `Wireless/RIS` when the title matches the regex
  `near[- ]field` case-insensitively, or the abstract contains `channel`
  case-insensitively, or any tag contains `RIS` case-insensitively.
- `doi-prefix` adds `Publishers/IEEE` when the DOI contains `10.1109`
  case-insensitively.
- `webpage` adds `Sources/Web` when the item type contains `webpage`
  case-insensitively, or the URI matches an HTTP/HTTPS URL regex.
- If an imported item matches multiple rules, all matching target categories
  are added.
- If multiple matching rules target the same category, that category is added
  once.
