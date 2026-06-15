//! Plugin-facing desktop notification endpoint.
//!
//! Plugins run as separate processes and reach the host only over REST, so a
//! plugin asks for a desktop notification with `POST /api/notify`. `core` must
//! not depend on the native layer, so this router is built in the binary and
//! merged into the app alongside `core`'s router.
//!
//! Delivery is decoupled from the request: the handler pushes a
//! [`NotifyRequest`] onto a process-global channel and a dedicated consumer
//! thread calls [`native_win32::show_app_notification`]. The consumer is the
//! one long-lived thread that touches the native notification API; it logs a
//! warning (rather than failing the request) when the platform cannot deliver
//! — covering non-Windows builds and missing Windows App SDK support.

use std::sync::OnceLock;
use std::sync::mpsc::{SyncSender, sync_channel};

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};

/// Process-global sender into the notification consumer thread.
///
/// Set once by [`start_notify_consumer`]; absent when notifications were never
/// started (e.g. a one-shot CLI invocation), which the handler reports as
/// `503` so a plugin can degrade gracefully.
static NOTIFY_TX: OnceLock<SyncSender<NotifyRequest>> = OnceLock::new();

/// Severity of a plugin-requested notification.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
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

impl NotifyKind {
    /// Map to the native notification severity.
    fn to_native(self) -> native_win32::NotificationKind {
        match self {
            Self::Info => native_win32::NotificationKind::Info,
            Self::Success => native_win32::NotificationKind::Success,
            Self::Error => native_win32::NotificationKind::Error,
        }
    }
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
/// owns all calls into the native notification API; the channel is bounded so
/// a flood of requests applies back-pressure rather than growing unboundedly.
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
            for request in rx {
                deliver(&request);
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

/// Deliver one notification through the native layer, logging on failure.
fn deliver(request: &NotifyRequest) {
    if let Err(error) = native_win32::show_app_notification(
        &request.title,
        &request.body,
        request.kind.to_native(),
    ) {
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
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "notifications unavailable",
        )
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
        let parsed: NotifyRequest = serde_json::from_str(
            r#"{"title":"t","body":"b"}"#,
        )
        .unwrap();
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
