use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::AuthState;
use domain_auth::ports::postgres::PostgresUserRepository;
use domain_notification::ports::events::NotificationSubscriber;
use domain_notification::ports::notifier::LogNotifier;
use domain_notification::ports::postgres::PostgresSentNotificationRepository;
use domain_notification::ports::repository::SentNotificationRepository;
use domain_notification::ports::templates::Templates;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::{
    dispatch_subscriber_once, DispatcherConfig, EventPublisher, OutboxPublisher, Routes,
};
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("../../domain-auth/tests/fixtures/test_priv.pem");

#[sqlx::test(migrations = "../../migrations")]
async fn register_dispatches_to_account_then_notification(pool: sqlx::PgPool) {
    let user_repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let account_repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let notif_repo = Arc::new(PostgresSentNotificationRepository::new(pool.clone()));
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(
        Routes::new()
            .add("user.registered", "account.on-user-registered")
            .add("account.created", "notification.on-account-created"),
    ));
    let account_sub = Arc::new(AccountSubscriber::new(
        pool.clone(),
        account_repo.clone(),
        publisher.clone(),
    ));
    let notif_sub = Arc::new(NotificationSubscriber::new(
        notif_repo.clone(),
        Arc::new(LogNotifier),
        Arc::new(Templates::new().unwrap()),
    ));

    // Register a user via the auth router (publishes user.registered).
    let auth = domain_auth::ports::http::router(AuthState {
        pool: pool.clone(),
        users: user_repo.clone(),
        refresh_tokens: user_repo.clone(),
        scopes: user_repo.clone(),
        publisher: publisher.clone(),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(
            JwtVerifier::from_rsa_pem(include_str!(
                "../../domain-auth/tests/fixtures/test_pub.pem"
            ))
            .unwrap(),
        ),
        revocation: Arc::new(NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: platform::metrics::Metrics::new().unwrap(),
    });
    let res = auth
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"e2e@x.y","password":"pw"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // 1st dispatch: account.on-user-registered creates the account + emits account.created.
    dispatch_subscriber_once(&pool, account_sub.as_ref(), &DispatcherConfig::default())
        .await
        .unwrap();
    // 2nd dispatch: notification.on-account-created consumes account.created.
    dispatch_subscriber_once(&pool, notif_sub.as_ref(), &DispatcherConfig::default())
        .await
        .unwrap();

    // A welcome notification was recorded for the new account's email.
    let sent = notif_repo.list().await.unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].recipient, "e2e@x.y");
    assert_eq!(sent[0].template, "welcome");
}
