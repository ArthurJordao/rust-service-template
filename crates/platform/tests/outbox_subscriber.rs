use platform::events::{
    dispatch_subscriber_once, ConsumerConfig, DeliveredEvent, DispatcherConfig, EventPublisher,
    NewEvent, OutboxPublisher, Routes, Subscriber,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<i64>>>);
#[async_trait::async_trait]
impl Subscriber for Recorder {
    fn name(&self) -> &'static str {
        "recorder"
    }
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
    async fn handle(&self, e: &DeliveredEvent) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(e.event_id);
        Ok(())
    }
    fn consumer_config(&self) -> ConsumerConfig {
        // Serial so the recorded order is deterministic (id order).
        ConsumerConfig {
            concurrency: 1,
            ..ConsumerConfig::default()
        }
    }
}

async fn publish(pool: &sqlx::PgPool, agg: &str) {
    let publisher = OutboxPublisher::new(Routes::new().add("user.registered", "recorder"));
    let mut tx = pool.begin().await.unwrap();
    publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: agg.into(),
                payload: serde_json::json!({}),
                correlation_id: "cid".into(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn delivers_pending_in_order_and_marks_delivered(pool: sqlx::PgPool) {
    let rec = Recorder::default();
    publish(&pool, "1").await;
    publish(&pool, "2").await;
    publish(&pool, "3").await;

    let n = dispatch_subscriber_once(&pool, &rec, &DispatcherConfig::default())
        .await
        .unwrap();
    assert_eq!(n, 3);

    let recorded = rec.0.lock().unwrap().clone();
    assert_eq!(recorded.len(), 3);
    let mut sorted = recorded.clone();
    sorted.sort();
    assert_eq!(recorded, sorted, "concurrency=1 should deliver in id order");

    let delivered: i64 =
        sqlx::query_scalar("select count(*) from outbox_delivery where status = 'delivered'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(delivered, 3);
}

struct AlwaysFails;
#[async_trait::async_trait]
impl Subscriber for AlwaysFails {
    fn name(&self) -> &'static str {
        "recorder"
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
    publish(&pool, "1").await;
    let config = DispatcherConfig { max_attempts: 2 };

    dispatch_subscriber_once(&pool, &AlwaysFails, &config)
        .await
        .unwrap(); // attempt 1 -> retry
    sqlx::query("update outbox_delivery set next_attempt_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    dispatch_subscriber_once(&pool, &AlwaysFails, &config)
        .await
        .unwrap(); // attempt 2 -> dead

    let row: (String, i32) = sqlx::query_as("select status, attempts from outbox_delivery limit 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "dead");
    assert_eq!(row.1, 2);
}
