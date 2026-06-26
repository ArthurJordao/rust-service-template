use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use platform::observability::{correlation_id_middleware, CORRELATION_ID_HEADER};
use tower::ServiceExt;

fn app() -> Router {
    Router::new()
        .route("/x", get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(correlation_id_middleware))
}

#[tokio::test]
async fn appends_a_segment_to_the_incoming_cid() {
    let res = app()
        .oneshot(
            Request::builder()
                .uri("/x")
                .header(CORRELATION_ID_HEADER, "root")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let echoed = res
        .headers()
        .get(CORRELATION_ID_HEADER)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        echoed.starts_with("root."),
        "expected child of root, got {echoed}"
    );
    assert_eq!(echoed.matches('.').count(), 1);
}

#[tokio::test]
async fn mints_a_root_when_no_header() {
    let res = app()
        .oneshot(Request::builder().uri("/x").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let echoed = res
        .headers()
        .get(CORRELATION_ID_HEADER)
        .unwrap()
        .to_str()
        .unwrap();
    // new root segment + appended child = two dotted segments
    assert_eq!(echoed.matches('.').count(), 1, "got {echoed}");
}
