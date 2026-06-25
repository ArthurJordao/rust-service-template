use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::ports::AccountRepository;
use platform::events::{DeliveredEvent, EventPublisher, OutboxPublisher, Routes, Subscriber};
use std::sync::Arc;

#[sqlx::test(migrations = "../../migrations")]
async fn subscriber_creates_account_from_user_registered(pool: sqlx::PgPool) {
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(Routes::new()));
    let repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let sub = AccountSubscriber::new(pool.clone(), repo.clone(), publisher);

    let event = DeliveredEvent {
        event_id: 1,
        event_type: "user.registered".into(),
        aggregate_id: "55".into(),
        payload: serde_json::json!({ "auth_user_id": 55, "email": "x@y.z" }),
        correlation_id: "cid-9".into(),
    };
    sub.handle(&event).await.unwrap();

    let acc = repo.find_by_auth_user_id(55).await.unwrap().unwrap();
    assert_eq!(acc.email, "x@y.z");
    assert_eq!(acc.created_by_cid, "cid-9");

    // Idempotent: handling the same event again does not duplicate.
    sub.handle(&event).await.unwrap();
    assert_eq!(repo.list().await.unwrap().len(), 1);
}
