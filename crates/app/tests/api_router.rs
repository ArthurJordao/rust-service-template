use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::http::AccountState;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::AuthState;
use domain_auth::ports::postgres::PostgresUserRepository;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::dlq_http::DlqState;
use platform::events::{OutboxPublisher, Routes};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_pub.pem");

fn build(pool: sqlx::PgPool) -> axum::Router {
    let metrics = Metrics::new().unwrap();
    let jwt = Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap());
    let revocation: Arc<dyn platform::auth::RevocationChecker> = Arc::new(NoopRevocationChecker);
    let user_repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let account = AccountState {
        pool: pool.clone(),
        repo: Arc::new(PostgresAccountRepository::new(pool.clone())),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        jwt: jwt.clone(),
        metrics: metrics.clone(),
        revocation: revocation.clone(),
    };
    let auth = AuthState {
        pool: pool.clone(),
        users: user_repo.clone(),
        refresh_tokens: user_repo.clone(),
        scopes: user_repo.clone(),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: jwt.clone(),
        revocation: revocation.clone(),
        admin_emails: Arc::new(vec![]),
        metrics: metrics.clone(),
    };
    let dlq = DlqState {
        pool: pool.clone(),
        jwt: jwt.clone(),
        revocation: revocation.clone(),
    };
    let notification = domain_notification::NotificationState {
        repo: Arc::new(
            domain_notification::ports::postgres::PostgresSentNotificationRepository::new(
                pool.clone(),
            ),
        ),
        jwt: jwt.clone(),
        revocation: revocation.clone(),
        metrics: metrics.clone(),
    };
    app::state::build_router(account, auth, dlq, notification, metrics, &[], None)
}

#[sqlx::test(migrations = "../../migrations")]
async fn status_at_root_and_api_routes_mounted(pool: sqlx::PgPool) {
    let app = build(pool);

    // /status at root -> 200
    let s = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(s.status(), StatusCode::OK);

    // API mounted under /api: an admin route with no token -> 401 (proves auth router mounted under /api)
    let scopes = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/scopes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(scopes.status(), StatusCode::UNAUTHORIZED);

    // DLQ mounted under /api -> 401 without token
    let dlq = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/admin/dlq")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dlq.status(), StatusCode::UNAUTHORIZED);

    // login is reachable under /api (bad body -> 400/422, NOT 404) — proves no route collision
    let login = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(login.status(), StatusCode::NOT_FOUND);
}
