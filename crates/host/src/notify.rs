//! Plugin-facing desktop notification endpoint.
//!
//! Plugins run as separate processes and reach the host only over REST, so a
//! plugin asks for a desktop notification with `POST /api/notify`. `core` must
//! not depend on the notification layer, so this router is built in the host and
//! merged into the app alongside `core`'s router.
//!
//! Delivery is decoupled from the request: the handler pushes a
//! [`NotifyRequest`] onto a process-global channel and a dedicated consumer
//! thread shows it via the cross-platform [`user_notify`] crate. That thread is
//! the one long-lived owner of the notification manager and its Tokio runtime
//! (the crate's `send_notification` is async); it logs a warning rather than
//! failing the request when a platform cannot deliver.

use std::sync::OnceLock;
use std::sync::mpsc::{SyncSender, sync_channel};

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use user_notify::{NotificationBuilder, get_notification_manager};

/// Windows AppUserModelID / notification identity for Localref.
///
/// On Windows this is the toast `app_id`; on macOS/Linux it is unused by the
/// notification manager. Kept stable so delivered toasts group under one app.
const APP_ID: &str = "com.localref.Localref.Desktop";

/// Process-global sender into the notification consumer thread.
///
/// Set once by [`start_notify_consumer`]; absent when notifications were never
/// started (e.g. a one-shot CLI invocation), which the handler reports as
/// `503` so a plugin can degrade gracefully.
static NOTIFY_TX: OnceLock<SyncSender<NotifyRequest>> = OnceLock::new();

/// Severity of a plugin-requested notification.
///
/// `user-notify` toasts do not carry a severity, so the kind is surfaced only
/// in the unified log line; it is retained on the wire for forward
/// compatibility and so plugins keep a stable request shape.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum NotifyKind {
    /// Informational notification.
    #[default]
    Info,
    /// Successful-operation notification.
    Success,
    /// Error notification.
    Error,
}

/// Request body for `POST /api/notify`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct NotifyRequest {
    /// Notification title.
    pub title: String,
    /// Notification body.
    pub body: String,
    /// Severity; defaults to `info` when omitted.
    #[serde(default)]
    pub kind: NotifyKind,
}

/// Start the notification consumer thread and register its sender.
///
/// Idempotent: a second call is a no-op once the sender is set. The consumer
/// owns the notification manager, registers the app, and (on macOS) requests
/// permission; the channel is bounded so a flood of requests applies
/// back-pressure rather than growing unboundedly.
pub fn start_notify_consumer() {
    if NOTIFY_TX.get().is_some() {
        return;
    }
    let (tx, rx) = sync_channel::<NotifyRequest>(256);
    if NOTIFY_TX.set(tx).is_err() {
        // Another thread won the race; its consumer is running.
        return;
    }
    let spawn = std::thread::Builder::new()
        .name("localref-notify".to_string())
        .spawn(move || {
            #[cfg(windows)]
            if let Err(error) = register_windows_notification_identity() {
                tracing::warn!(
                    target: "localref::notify",
                    %error,
                    "failed to register Windows notification identity",
                );
            }
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::warn!(
                        target: "localref::notify",
                        %error,
                        "failed to build notification runtime; notifications disabled",
                    );
                    return;
                }
            };
            let manager =
                get_notification_manager(APP_ID.to_string(), None);
            // Register delivery handling and request permission where required.
            // Failures degrade to best-effort delivery rather than aborting.
            if let Err(error) = manager.register(Box::new(|_response| {}), Vec::new())
            {
                tracing::warn!(
                    target: "localref::notify",
                    %error,
                    "failed to register notification manager",
                );
            } else {
                tracing::info!(
                    target: "localref::notify",
                    app_id = APP_ID,
                    "notification manager registered",
                );
            }
            if let Err(error) = runtime
                .block_on(manager.first_time_ask_for_notification_permission())
            {
                tracing::warn!(
                    target: "localref::notify",
                    %error,
                    "failed to request notification permission",
                );
            }
            for request in rx {
                deliver(&runtime, manager.as_ref(), &request);
            }
        });
    if let Err(error) = spawn {
        tracing::warn!(
            target: "localref::notify",
            %error,
            "failed to start notification consumer thread",
        );
    }
}

/// Register the unpackaged desktop app identity Windows requires before an
/// arbitrary AppUserModelID can create toast notifications.
#[cfg(windows)]
fn register_windows_notification_identity() -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let app_id = wide(APP_ID);
    // SAFETY: `app_id` is a valid, NUL-terminated UTF-16 string for the
    // duration of the call. The API only reads it and updates process state.
    let result =
        unsafe { SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr()) };
    if result < 0 {
        return Err(format!(
            "SetCurrentProcessExplicitAppUserModelID failed: 0x{result:08X}"
        ));
    }

    Ok(())
}

/// Deliver one notification via `user-notify`, logging on failure.
fn deliver(
    runtime: &tokio::runtime::Runtime,
    manager: &dyn user_notify::NotificationManager,
    request: &NotifyRequest,
) {
    let notification =
        NotificationBuilder::new().title(&request.title).body(&request.body);
    if let Err(error) =
        runtime.block_on(manager.send_notification(notification))
    {
        tracing::warn!(
            target: "localref::notify",
            %error,
            title = %request.title,
            "failed to show desktop notification",
        );
    }
}

/// Build the `/api/notify` router.
pub fn notify_router() -> Router {
    Router::new().route("/api/notify", post(notify))
}

/// Handle `POST /api/notify`: enqueue the request for the consumer thread.
///
/// Mirrors the request to the unified log so notifications are visible there
/// too, returns `204` when enqueued, and `503` when no consumer is running
/// (notifications unavailable in this process).
async fn notify(Json(request): Json<NotifyRequest>) -> Response {
    let Some(tx) = NOTIFY_TX.get() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "notifications unavailable")
            .into_response();
    };
    tracing::info!(
        target: "localref::notify",
        title = %request.title,
        kind = ?request.kind,
        "{}",
        request.body,
    );
    match tx.try_send(request) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::warn!(
                target: "localref::notify",
                %error,
                "dropping notification; consumer queue is full or closed",
            );
            (StatusCode::SERVICE_UNAVAILABLE, "notification queue full")
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NotifyKind, NotifyRequest, notify_router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn notify_kind_defaults_to_info() {
        let parsed: NotifyRequest =
            serde_json::from_str(r#"{"title":"t","body":"b"}"#).unwrap();
        assert_eq!(parsed.kind, NotifyKind::Info);
    }

    #[test]
    fn notify_kind_parses_lowercase_variants() {
        let parsed: NotifyRequest = serde_json::from_str(
            r#"{"title":"t","body":"b","kind":"success"}"#,
        )
        .unwrap();
        assert_eq!(parsed.kind, NotifyKind::Success);
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_identity_registration_succeeds() {
        super::register_windows_notification_identity()
            .expect("register Windows AppUserModelID");
    }

    #[tokio::test]
    async fn notify_returns_503_when_consumer_absent() {
        // This test never starts the consumer, so NOTIFY_TX is unset and the
        // handler must report the capability as unavailable rather than panic.
        let app = notify_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/notify")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"title":"hi","body":"there"}"#.to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
