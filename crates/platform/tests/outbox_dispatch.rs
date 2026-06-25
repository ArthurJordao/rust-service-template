use platform::events::{
    dispatch_once, DeliveredEvent, DispatcherConfig, EventPublisher, NewEvent, OutboxPublisher,
    Routes, Subscriber, SubscriberRegistry,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);
#[async_trait::async_trait]
impl Subscriber for Recorder {
    fn name(&self) -> &'static str {
        "recorder"
    }
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
    async fn handle(&self, e: &DeliveredEvent) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(e.correlation_id.clone());
        Ok(())
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn dispatch_delivers_pending_and_marks_delivered(pool: sqlx::PgPool) {
    let rec = Recorder::default();
    let mut reg = SubscriberRegistry::new();
    reg.register(Arc::new(rec.clone()));
    let reg = Arc::new(reg);

    let publisher = OutboxPublisher::new(Routes::new().add("user.registered", "recorder"));
    let mut tx = pool.begin().await.unwrap();
    publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: "7".into(),
                payload: serde_json::json!({}),
                correlation_id: "cid-xyz".into(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let n = dispatch_once(&pool, &reg, &DispatcherConfig::default())
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(rec.0.lock().unwrap().as_slice(), &["cid-xyz".to_string()]);

    let delivered: i64 =
        sqlx::query_scalar("select count(*) from outbox_delivery where status = 'delivered'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(delivered, 1);
}

struct AlwaysFails;
#[async_trait::async_trait]
impl Subscriber for AlwaysFails {
    fn name(&self) -> &'static str {
        "always-fails"
    }
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
    async fn handle(&self, _e: &DeliveredEvent) -> anyhow::Result<()> {
        anyhow::bail!("boom")
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn failing_delivery_dead_letters_after_max_attempts(pool: sqlx::PgPool) {
    let mut reg = SubscriberRegistry::new();
    reg.register(Arc::new(AlwaysFails));
    let reg = Arc::new(reg);

    let publisher = OutboxPublisher::new(Routes::new().add("user.registered", "always-fails"));
    let mut tx = pool.begin().await.unwrap();
    publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: "1".into(),
                payload: serde_json::json!({}),
                correlation_id: "cid".into(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // max_attempts = 2 so it dead-letters fast; reset next_attempt_at between runs.
    let config = DispatcherConfig { max_attempts: 2, batch_size: 50 };

    dispatch_once(&pool, &reg, &config).await.unwrap(); // attempt 1 -> retry scheduled
    sqlx::query("update outbox_delivery set next_attempt_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    dispatch_once(&pool, &reg, &config).await.unwrap(); // attempt 2 -> dead

    let row: (String, i32) =
        sqlx::query_as("select status, attempts from outbox_delivery limit 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "dead");
    assert_eq!(row.1, 2);
}
