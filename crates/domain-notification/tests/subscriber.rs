use domain_notification::ports::events::NotificationSubscriber;
use domain_notification::ports::notifier::LogNotifier;
use domain_notification::ports::postgres::PostgresSentNotificationRepository;
use domain_notification::ports::repository::SentNotificationRepository;
use domain_notification::ports::templates::Templates;
use platform::events::{DeliveredEvent, Subscriber};
use std::sync::Arc;

fn event(event_id: i64, email: &str) -> DeliveredEvent {
    DeliveredEvent {
        event_id,
        event_type: "account.created".into(),
        aggregate_id: "1".into(),
        payload: serde_json::json!({ "account_id": 1, "auth_user_id": 1, "email": email }),
        correlation_id: "root.ab.cd".into(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn sends_and_records_then_is_idempotent(pool: sqlx::PgPool) {
    let repo = Arc::new(PostgresSentNotificationRepository::new(pool.clone()));
    let sub = NotificationSubscriber::new(
        repo.clone(),
        Arc::new(LogNotifier),
        Arc::new(Templates::new().unwrap()),
    );

    let e = event(10, "a@b.c");
    sub.handle(&e).await.unwrap();
    let row = repo.find_by_event_id(10).await.unwrap().unwrap();
    assert_eq!(row.recipient, "a@b.c");
    assert_eq!(row.template, "welcome");
    assert_eq!(row.subject, "Welcome");
    assert_eq!(row.created_by_cid, "root.ab.cd");

    // Redelivery is a no-op.
    sub.handle(&e).await.unwrap();
    assert_eq!(repo.list().await.unwrap().len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn fail_test_recipient_errors_for_dlq(pool: sqlx::PgPool) {
    let repo = Arc::new(PostgresSentNotificationRepository::new(pool.clone()));
    let sub = NotificationSubscriber::new(
        repo,
        Arc::new(LogNotifier),
        Arc::new(Templates::new().unwrap()),
    );
    assert!(sub.handle(&event(11, "boom@fail.test")).await.is_err());
}
