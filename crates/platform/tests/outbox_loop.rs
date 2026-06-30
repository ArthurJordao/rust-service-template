use platform::events::{
    run_subscriber_loop, ConsumerConfig, DeliveredEvent, DispatcherConfig, EventPublisher,
    NewEvent, OutboxPublisher, Routes, Subscriber,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

struct FastRecorder(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Subscriber for FastRecorder {
    fn name(&self) -> &'static str {
        "recorder"
    }
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
    async fn handle(&self, _e: &DeliveredEvent) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn consumer_config(&self) -> ConsumerConfig {
        ConsumerConfig {
            poll_interval: Duration::from_millis(50),
            ..ConsumerConfig::default()
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn loop_drains_pending(pool: sqlx::PgPool) {
    let count = Arc::new(AtomicUsize::new(0));
    let sub: Arc<dyn Subscriber> = Arc::new(FastRecorder(count.clone()));

    let publisher = OutboxPublisher::new(Routes::new().add("user.registered", "recorder"));
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

    let handle = tokio::spawn(run_subscriber_loop(
        pool.clone(),
        sub,
        DispatcherConfig::default(),
    ));

    // Poll up to ~3s for the loop to process the one delivery.
    let mut delivered = false;
    for _ in 0..30 {
        if count.load(Ordering::SeqCst) == 1 {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handle.abort();
    assert!(delivered, "loop did not drain the pending delivery");
}
