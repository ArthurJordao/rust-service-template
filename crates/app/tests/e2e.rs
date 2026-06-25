use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::http::{router, AccountState};
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::ports::AccountRepository;
use platform::auth::JwtVerifier;
use platform::events::{
    dispatch_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry,
};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PUB_PEM: &str = include_str!("../../domain-account/tests/fixtures/test_pub.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn dev_register_then_dispatch_creates_account(pool: sqlx::PgPool) {
    // Build publisher -> subscriber -> registry (mirrors app wiring; linear, no cycle).
    let repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(
        Routes::new().add("user.registered", "account.on-user-registered"),
    ));
    let mut registry = SubscriberRegistry::new();
    registry.register(Arc::new(AccountSubscriber::new(
        pool.clone(),
        repo.clone(),
        publisher.clone(),
    )));
    let registry = Arc::new(registry);

    let state = AccountState {
        pool: pool.clone(),
        repo: repo.clone(),
        publisher: publisher.clone(),
        jwt: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        metrics: Metrics::new().unwrap(),
    };
    let app = router(state);

    // 1. POST /dev/register publishes user.registered.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/dev/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"auth_user_id":77,"email":"e2e@x.y"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    // 2. Dispatcher delivers the event -> account subscriber creates account.
    dispatch_once(&pool, &registry, &DispatcherConfig::default())
        .await
        .unwrap();

    // 3. Account now exists, and account.created was emitted.
    let acc = repo.find_by_auth_user_id(77).await.unwrap();
    assert!(acc.is_some(), "account created by event handler");

    let created: i64 = sqlx::query_scalar(
        "select count(*) from outbox_event where event_type = 'account.created'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(created, 1);
}
