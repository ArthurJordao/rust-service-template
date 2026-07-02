use platform::events::{
    run_reaper, run_subscriber_loop, ConsumerConfig, DeliveredEvent, DispatcherConfig,
    EventPublisher, NewEvent, OutboxPublisher, ReaperConfig, Routes, Subscriber,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

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
async fn cancelling_consumer_drains_processing_and_exits(pool: sqlx::PgPool) {
    let count = Arc::new(AtomicUsize::new(0));
    let sub: Arc<dyn Subscriber> = Arc::new(FastRecorder(count.clone()));

    let publisher = OutboxPublisher::new(Routes::new().add("user.registered", "recorder"));
    for _ in 0..3 {
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
    }

    let token = CancellationToken::new();
    let handle = tokio::spawn(run_subscriber_loop(
        pool.clone(),
        sub,
        DispatcherConfig::default(),
        token.clone(),
    ));

    // Let at least one cycle run, then request shutdown.
    for _ in 0..30 {
        if count.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    token.cancel();

    // The loop must return promptly after cancellation.
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("loop did not exit after cancel")
        .unwrap();

    // Key invariant: no row left mid-flight in `processing`.
    let processing: i64 =
        sqlx::query_scalar("select count(*) from outbox_delivery where status = 'processing'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(processing, 0, "a batch was abandoned mid-flight");
}

#[sqlx::test(migrations = "../../migrations")]
async fn cancelling_reaper_exits(pool: sqlx::PgPool) {
    let token = CancellationToken::new();
    let handle = tokio::spawn(run_reaper(
        pool.clone(),
        ReaperConfig {
            poll_interval: Duration::from_millis(50),
            ..ReaperConfig::default()
        },
        5,
        token.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(60)).await;
    token.cancel();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("reaper did not exit after cancel")
        .unwrap();
}
