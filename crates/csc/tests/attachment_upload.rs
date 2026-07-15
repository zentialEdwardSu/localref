//! The connector uploads attachment bytes directly to `/connector/saveAttachment`
//! with the raw file as the request body. Axum's `Bytes` extractor enforces a
//! 2 MB default body limit, which silently rejected every larger PDF/EPUB with
//! `413` before the handler ran — the cause of the ~80% attachment failure.
//! These tests pin that a large upload now reaches the sink and is accepted.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use csc::{MemoryImportSink, router};
use tower::ServiceExt as _;

/// A payload comfortably larger than axum's 2 MB default body limit.
const LARGE_BODY_LEN: usize = 8 * 1024 * 1024;

fn save_attachment_request(body_len: usize) -> Request<Body> {
    let metadata = serde_json::json!({
        "url": "https://example.org/paper.pdf",
        "contentType": "application/pdf",
        "parentItemID": "paper-1",
        "title": "Paper",
    })
    .to_string();
    Request::builder()
        .method("POST")
        .uri("/connector/saveAttachment?sessionID=session-1")
        .header("Content-Type", "application/pdf")
        .header("X-Metadata", metadata)
        .body(Body::from(vec![0u8; body_len]))
        .unwrap()
}

#[tokio::test]
async fn accepts_attachment_larger_than_default_body_limit() {
    let sink = Arc::new(MemoryImportSink::default());
    let app = router(sink.clone());

    let response =
        app.oneshot(save_attachment_request(LARGE_BODY_LEN)).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "a >2MB attachment upload must be accepted, not rejected with 413",
    );
    let attachments = sink.attachments();
    assert_eq!(attachments.len(), 1, "the sink must receive the upload");
    assert_eq!(
        attachments[0].bytes.len(),
        LARGE_BODY_LEN,
        "the full body must reach the sink intact",
    );
}
