use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_notification::ports::http::{router, NotificationState};
use domain_notification::ports::postgres::PostgresSentNotificationRepository;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn list_notifications_without_token_is_unauthorized(pool: sqlx::PgPool) {
    let state = NotificationState {
        repo: Arc::new(PostgresSentNotificationRepository::new(pool)),
        jwt: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(NoopRevocationChecker),
        metrics: Metrics::new().unwrap(),
    };
    let app = router(state);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/notifications")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
