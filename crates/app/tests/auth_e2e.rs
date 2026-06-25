use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::ports::AccountRepository;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::{router, AuthState};
use domain_auth::ports::postgres::PostgresUserRepository;
use platform::events::{
    dispatch_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry,
};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_pub.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn register_then_dispatch_creates_account(pool: sqlx::PgPool) {
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

    let auth_repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let auth = router(AuthState {
        pool: pool.clone(),
        users: auth_repo.clone(),
        refresh_tokens: auth_repo.clone(),
        scopes: auth_repo.clone(),
        publisher: publisher.clone(),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(platform::auth::JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(platform::auth::NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
    });

    // 1. Register publishes user.registered.
    let res = auth
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"e2e@x.y","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // 2. Dispatcher delivers it -> account subscriber creates the account.
    dispatch_once(&pool, &registry, &DispatcherConfig::default())
        .await
        .unwrap();

    // 3. The account now exists (auth_user.id == auth_user_id == 1 for the first user).
    let acc = account_repo.find_by_auth_user_id(1).await.unwrap();
    assert!(acc.is_some(), "account created from user.registered");
}
