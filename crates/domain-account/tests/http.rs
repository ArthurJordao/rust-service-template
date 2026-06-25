use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::http::{router, AccountState};
use domain_account::ports::postgres::PostgresAccountRepository;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::{OutboxPublisher, Routes};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

// A minimal valid RSA public key PEM is required to build a verifier; for these
// tests we only exercise unauthenticated paths, so any well-formed PEM works.
const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

fn state(pool: sqlx::PgPool) -> AccountState {
    AccountState {
        pool: pool.clone(),
        repo: Arc::new(PostgresAccountRepository::new(pool)),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        jwt: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        metrics: Metrics::new().unwrap(),
        revocation: Arc::new(NoopRevocationChecker),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn status_returns_ok(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let res = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_account_without_token_is_unauthorized(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let res = app
        .oneshot(
            Request::builder()
                .uri("/accounts/1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn account_me_without_token_is_unauthorized(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let res = app
        .oneshot(
            Request::builder()
                .uri("/accounts/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
