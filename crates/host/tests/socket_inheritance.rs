//! Regression test for the reported port-leak: a plugin child process must not
//! keep the daemon's listening port bound after the listener is closed.
//!
//! On Windows, tokio/mio create listening sockets as inheritable, and plugin
//! children are spawned with piped stdio (`bInheritHandles=TRUE`), so a child
//! inherits the daemon's REST/CSC socket and holds the port open even after the
//! parent exits. `deny_socket_inheritance` clears the inherit flag so the child
//! can never inherit the socket; this test proves the port frees immediately
//! once the listener is dropped, even while such a child is still alive.

use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};

use localref_host::server::deny_socket_inheritance;

/// Spawn a process that stays alive for a few seconds with inheritable stdio,
/// exactly as the plugin invoker does (piped stdout/stderr → `bInheritHandles`).
fn spawn_long_child() -> Child {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        let _ = c.args(["/C", "ping", "-n", "5", "127.0.0.1"]);
        c
    } else {
        let mut c = Command::new("sleep");
        let _ = c.arg("5");
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child")
}

#[test]
fn child_does_not_pin_port_after_listener_closes() {
    // Bind an ephemeral port on the tokio runtime the way the servers do, then
    // clear the inherit flag before spawning a child.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("bind");
    let addr = listener.local_addr().expect("addr");

    deny_socket_inheritance(&listener);

    // A child spawned now must NOT inherit the (now non-inheritable) socket.
    let mut child = spawn_long_child();

    // Close the listener while the child is still running.
    drop(listener);

    // The port must be immediately re-bindable: nothing else holds the socket.
    // Without the fix, on Windows the surviving child would keep it bound and
    // this bind would fail with AddrInUse.
    let rebound = StdTcpListener::bind(addr);
    let bind_result = rebound.is_ok();

    // Clean up the child regardless of the assertion outcome.
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        bind_result,
        "port {addr} was still bound after closing the listener — a child \
         inherited the socket (port leak)",
    );
}
