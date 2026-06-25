use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::http::{router as account_router, AccountState};
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::{router as auth_router, AuthState};
use domain_auth::ports::postgres::PostgresUserRepository;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::{OutboxPublisher, Routes};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_pub.pem");

fn account_state(pool: sqlx::PgPool) -> AccountState {
    AccountState {
        pool: pool.clone(),
        repo: Arc::new(PostgresAccountRepository::new(pool)),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        jwt: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        metrics: Metrics::new().unwrap(),
        revocation: Arc::new(NoopRevocationChecker),
    }
}

fn auth_state(pool: sqlx::PgPool) -> AuthState {
    let repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    AuthState {
        pool: pool.clone(),
        users: repo.clone(),
        refresh_tokens: repo.clone(),
        scopes: repo.clone(),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
    }
}

/// Verify that merging the two domain routers does not panic (no overlapping routes)
/// and that routes from both domains are correctly mounted.
#[sqlx::test(migrations = "../../migrations")]
async fn merged_router_boots_and_routes_both_domains(pool: sqlx::PgPool) {
    // This merge must NOT panic — if /status or /metrics overlap, axum 0.7 panics here.
    let app = account_router(account_state(pool.clone())).merge(auth_router(auth_state(pool)));

    // GET /status -> 200 (proves account routes are mounted and no overlap panic occurred)
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "GET /status should return 200"
    );

    // GET /scopes with no Authorization header -> 401 (proves auth routes are mounted and guarded)
    let res = app
        .oneshot(
            Request::builder()
                .uri("/scopes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "GET /scopes without auth should return 401"
    );
}
