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
