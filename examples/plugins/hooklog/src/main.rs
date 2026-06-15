//! Example Localref plugin demonstrating the `hook` and `cron` entry points.
//!
//! The host spawns this plugin automatically — after lifecycle events it
//! declared a hook for, and on its declared cron schedule. Each invocation
//! appends one line to a log file so the behaviour is observable end-to-end.
//!
//! ```text
//! hooklog hook item_imported --endpoint http://127.0.0.1:24817 --item lr:zotero:a
//! hooklog cron heartbeat --endpoint http://127.0.0.1:24817
//! ```
//!
//! The log file defaults to `<temp dir>/localref-hooklog.txt`; override it with
//! the `HOOKLOG_FILE` environment variable.

use std::fmt::Write as _;
use std::io::Write as _;

use localref_plugin_sdk::{Invocation, RunOutput, emit, parse_args};

#[tokio::main]
async fn main() {
    let Some(invocation) = parse_args(std::env::args().skip(1)) else {
        emit(&RunOutput::error(
            "usage: hooklog <hook|cron> … --endpoint …",
        ));
        return;
    };
    let line = match invocation {
        Invocation::Hook { event, item, category, .. } => {
            let mut line = format!("hook event={event}");
            if let Some(item) = item {
                let _ = write!(line, " item={item}");
            }
            if let Some(category) = category {
                let _ = write!(line, " category={category}");
            }
            line
        }
        Invocation::Cron { job, .. } => format!("cron job={job}"),
        Invocation::Run { action, .. } => format!("run action={action}"),
        Invocation::Manifest => {
            println!("hooklog — appends a line on each hook and cron run");
            return;
        }
    };
    emit(&append_line(&line));
}

/// Append one line to the log file, returning the result envelope.
fn append_line(line: &str) -> RunOutput {
    let path = std::env::var_os("HOOKLOG_FILE").map_or_else(
        || std::env::temp_dir().join("localref-hooklog.txt"),
        Into::into,
    );
    let opened =
        std::fs::OpenOptions::new().create(true).append(true).open(&path);
    match opened.and_then(|mut file| writeln!(file, "{line}")) {
        Ok(()) => RunOutput::ok(format!("logged: {line}")),
        Err(error) => RunOutput::error(format!("write failed: {error}")),
    }
}
