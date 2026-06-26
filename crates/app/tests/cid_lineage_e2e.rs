use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::http::AccountState;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::AuthState;
use domain_auth::ports::postgres::PostgresUserRepository;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::dlq_http::DlqState;
use platform::events::{
    dispatch_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry,
};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_pub.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn cid_lineage_grows_through_the_event_chain(pool: sqlx::PgPool) {
    let metrics = Metrics::new().unwrap();
    let jwt = Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap());
    let revocation: Arc<dyn platform::auth::RevocationChecker> = Arc::new(NoopRevocationChecker);
    let user_repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let account_repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(
        Routes::new().add("user.registered", "account.on-user-registered"),
    ));
    let mut registry = SubscriberRegistry::new();
    registry.register(Arc::new(AccountSubscriber::new(
        pool.clone(),
        account_repo.clone(),
        publisher.clone(),
    )));
    let registry = Arc::new(registry);

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
    let app = app::state::build_router(account, auth, dlq, notification, metrics, &[], None);

    // Register with an explicit root cid.
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/register")
                .header("content-type", "application/json")
                .header("x-correlation-id", "root")
                .body(Body::from(r#"{"email":"e2e@x.y","password":"pw"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // user.registered row cid extends "root" (middleware appended .a, publish appended .b).
    let ur_cid: String = sqlx::query_scalar(
        "select correlation_id from outbox_event where event_type = 'user.registered'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(ur_cid.starts_with("root."), "user.registered cid: {ur_cid}");
    assert!(
        ur_cid.matches('.').count() >= 2,
        "expected request+publish segments: {ur_cid}"
    );

    // Dispatch -> account subscriber runs under ur_cid and publishes account.created,
    // whose row cid extends ur_cid further.
    dispatch_once(&pool, &registry, &DispatcherConfig::default())
        .await
        .unwrap();
    let ac_cid: String = sqlx::query_scalar(
        "select correlation_id from outbox_event where event_type = 'account.created'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        ac_cid.starts_with(&format!("{ur_cid}.")),
        "account.created cid {ac_cid} must extend {ur_cid}"
    );
}
