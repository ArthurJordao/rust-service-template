use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::http::AccountState;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::AuthState;
use domain_auth::ports::postgres::PostgresUserRepository;
use http_body_util::BodyExt;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::dlq_http::DlqState;
use platform::events::{
    dispatch_subscriber_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes,
};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_pub.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn register_dispatch_then_get_my_account(pool: sqlx::PgPool) {
    let metrics = Metrics::new().unwrap();
    let jwt = Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap());
    let revocation: Arc<dyn platform::auth::RevocationChecker> = Arc::new(NoopRevocationChecker);
    let user_repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let account_repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(
        Routes::new().add("user.registered", "account.on-user-registered"),
    ));
    let account_sub = Arc::new(AccountSubscriber::new(
        pool.clone(),
        account_repo.clone(),
        publisher.clone(),
    ));

    let account = AccountState {
        pool: pool.clone(),
        repo: account_repo.clone(),
        publisher: publisher.clone(),
        jwt: jwt.clone(),
        metrics: metrics.clone(),
        revocation: revocation.clone(),
    };
    let auth = AuthState {
        pool: pool.clone(),
        users: user_repo.clone(),
        refresh_tokens: user_repo.clone(),
        scopes: user_repo.clone(),
        publisher: publisher.clone(),
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
    let app = app::state::build_router(
        account,
        auth,
        dlq,
        notification,
        metrics,
        pool.clone(),
        app::state::RouterConfig::new(vec![]),
        None,
    );

    // Register -> tokens
    let reg = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"me@x.y","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg.status(), StatusCode::CREATED);
    let body = reg.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let access = json["access_token"].as_str().unwrap().to_string();

    // Dispatch the user.registered -> account created
    dispatch_subscriber_once(&pool, account_sub.as_ref(), &DispatcherConfig::default())
        .await
        .unwrap();

    // GET /api/accounts/me with the access token -> 200 + the account
    let me = app
        .oneshot(
            Request::builder()
                .uri("/api/accounts/me")
                .header("authorization", format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body = me.into_body().collect().await.unwrap().to_bytes();
    let acc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(acc["email"], "me@x.y");
}
