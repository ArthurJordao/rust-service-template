use app::state::governor_layer;
use axum::body::Body;
use axum::routing::post;
use axum::Router;
use http::{Request, StatusCode};
use tower::ServiceExt;

fn app() -> Router {
    // 2 requests/period, burst 2, so the 3rd rapid request from one IP is limited.
    Router::new()
        .route("/login", post(|| async { "ok" }))
        .layer(governor_layer(120, 2)) // 120/min => period 0.5s; burst 2
}

fn req(ip: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/login")
        .header("fly-client-ip", ip)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn third_rapid_request_from_same_ip_is_limited() {
    let app = app();
    let s1 = app.clone().oneshot(req("1.1.1.1")).await.unwrap().status();
    let s2 = app.clone().oneshot(req("1.1.1.1")).await.unwrap().status();
    let s3 = app.clone().oneshot(req("1.1.1.1")).await.unwrap().status();
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(s3, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_different_ip_has_its_own_bucket() {
    let app = app();
    // exhaust 1.1.1.1
    let _ = app.clone().oneshot(req("1.1.1.1")).await.unwrap();
    let _ = app.clone().oneshot(req("1.1.1.1")).await.unwrap();
    let _ = app.clone().oneshot(req("1.1.1.1")).await.unwrap();
    // a different IP is unaffected
    let other = app.clone().oneshot(req("2.2.2.2")).await.unwrap().status();
    assert_eq!(other, StatusCode::OK);
}
