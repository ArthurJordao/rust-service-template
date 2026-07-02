use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use http::{Request, StatusCode};
use std::time::Duration;
use tower::ServiceExt;
use tower_http::timeout::TimeoutLayer;

#[tokio::test]
async fn timeout_layer_returns_408_on_slow_handler() {
    let app = Router::new()
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                "done"
            }),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_millis(100),
        ));

    let res = app
        .oneshot(Request::builder().uri("/slow").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::REQUEST_TIMEOUT);
}

#[tokio::test]
async fn body_limit_returns_413_over_cap() {
    let app = Router::new()
        .route("/echo", post(|_b: axum::body::Bytes| async { "ok" }))
        .layer(DefaultBodyLimit::max(16));

    let big = vec![b'x'; 64];
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/echo")
                .body(Body::from(big))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
