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
